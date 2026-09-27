#include "hz/detail/work_queue.hpp"

#include <climits>

#include <algorithm>
#include <sys/resource.h>
#include <thread>
#include <utility>

namespace hz::detail {

unsigned default_workers() {
    // Each thread keeps directories open as deep as the tree. When the
    // open-file limit cannot be raised far, walk on one thread.
    rlimit limit{};
    if (::getrlimit(RLIMIT_NOFILE, &limit) == 0 && limit.rlim_max < 4096) {
        return 1;
    }
    return std::clamp(std::thread::hardware_concurrency(), 1U, 4U);
}

WorkQueue::WorkQueue(unsigned workers) : workers_(std::clamp(workers, 1U, 64U)) {}

void WorkQueue::push(Job job) {
    {
        const std::scoped_lock lock(mutex_);
        if (failure_) {
            return;
        }
        jobs_.push_back(std::move(job));
    }
    changed_.notify_one();
}

namespace {

// Tree work keeps a few directories open per thread, and macOS starts
// processes with a soft limit of 256 descriptors. Raises the soft limit
// toward the hard limit while tree work runs; processes hz starts later
// inherit the original limit.
class RaisedFileLimit {
  public:
    RaisedFileLimit() {
        const std::scoped_lock lock(mutex);
        if (users++ > 0 || ::getrlimit(RLIMIT_NOFILE, &original) != 0) {
            return;
        }
        rlim_t wanted = std::min<rlim_t>(original.rlim_max, 65536);
#ifdef __APPLE__
        wanted = std::min<rlim_t>(wanted, OPEN_MAX);
#endif
        if (original.rlim_cur < wanted) {
            rlimit raised_limit = original;
            raised_limit.rlim_cur = wanted;
            raised = ::setrlimit(RLIMIT_NOFILE, &raised_limit) == 0;
        }
    }
    ~RaisedFileLimit() {
        const std::scoped_lock lock(mutex);
        if (--users == 0 && raised) {
            ::setrlimit(RLIMIT_NOFILE, &original);
            raised = false;
        }
    }
    RaisedFileLimit(const RaisedFileLimit&) = delete;
    RaisedFileLimit& operator=(const RaisedFileLimit&) = delete;
    RaisedFileLimit(RaisedFileLimit&&) = delete;
    RaisedFileLimit& operator=(RaisedFileLimit&&) = delete;

  private:
    // Shared by overlapping runs: the last to finish restores the limit.
    static inline std::mutex mutex;
    static inline unsigned users = 0;
    static inline rlimit original{};
    static inline bool raised = false;
};

} // namespace

void WorkQueue::run() {
    const RaisedFileLimit limit;
    std::vector<std::thread> threads;
    try {
        threads.reserve(workers_ - 1);
        for (unsigned i = 1; i < workers_; ++i) {
            threads.emplace_back([this] { work(); });
        }
    } catch (...) { // NOLINT(bugprone-empty-catch): fewer threads still finish the work
    }
    work();
    for (auto& thread : threads) {
        thread.join();
    }
    if (failure_) {
        std::rethrow_exception(failure_);
    }
}

void WorkQueue::work() {
    for (;;) {
        Job job;
        {
            std::unique_lock lock(mutex_);
            changed_.wait(lock, [this] { return !jobs_.empty() || running_ == 0; });
            if (jobs_.empty()) {
                return;
            }
            job = std::move(jobs_.back());
            jobs_.pop_back();
            ++running_;
        }
        std::exception_ptr failure;
        try {
            job();
        } catch (...) {
            failure = std::current_exception();
        }
        job = nullptr; // release what the job holds before others observe completion
        {
            const std::scoped_lock lock(mutex_);
            --running_;
            if (failure && !failure_) {
                failure_ = failure;
                jobs_.clear();
            }
        }
        changed_.notify_all();
    }
}

} // namespace hz::detail
