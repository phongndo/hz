#pragma once

#include <atomic>
#include <condition_variable>
#include <cstddef>
#include <exception>
#include <functional>
#include <memory>
#include <mutex>
#include <vector>

namespace hz::detail {

// Threads used for filesystem tree work unless the caller chooses. Metadata
// operations contend on filesystem locks; beyond a few threads, btrfs spends
// more time spinning than copying.
unsigned default_workers();

// Runs jobs, which may push more jobs, on a fixed number of threads. The first
// job to throw stops the queue: jobs not yet started are dropped and run()
// rethrows that exception once running jobs finish. Jobs are taken newest
// first, so a tree walk stays depth-first and keeps few directories open.
class WorkQueue {
  public:
    using Job = std::function<void()>;

    explicit WorkQueue(unsigned workers);

    void push(Job job);
    // Runs until no job is queued or running. With one worker, runs inline.
    void run();

  private:
    void work();

    unsigned workers_;
    std::mutex mutex_;
    std::condition_variable changed_;
    std::vector<Job> jobs_;
    std::size_t running_ = 0;
    std::exception_ptr failure_;
};

// Files per job in tree work, so one large directory is handled by several
// threads.
inline constexpr std::size_t file_batch = 256;

// Tree work tracks each directory with a node holding `pending`, a count of
// its unfinished parts, and a `parent` pointer. Counts one part of `node`
// finished; when none remain, calls `done` on it and continues with its
// parent, whose part it was.
template <typename Node, typename Done> void count_down(std::shared_ptr<Node> node, Done done) {
    while (node && node->pending.fetch_sub(1, std::memory_order_acq_rel) == 1) {
        done(*node);
        node = std::move(node->parent);
    }
}

} // namespace hz::detail
