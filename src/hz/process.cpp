#include "hz/process.hpp"

#include "hz/detail/fd.hpp"
#include "hz/error.hpp"

#include <array>
#include <cerrno>
#include <csignal>
#include <cstring>
#include <fcntl.h>
#include <format>
#include <map>
#include <poll.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>

// POSIX owns this mutable global; we only read it to build the child's env.
extern char**
    environ; // NOLINT(readability-redundant-declaration,cppcoreguidelines-avoid-non-const-global-variables)

namespace hz {

namespace {

void check_posix(int error, std::string_view operation) {
    if (error != 0) {
        throw Error(ErrorKind::io, std::format("{}: {}", operation, std::strerror(error)));
    }
}

class Pipe {
  public:
    Pipe() {
        if (::pipe(fds_.data()) != 0) {
            throw errno_error("create pipe", "");
        }
        for (int fd : fds_) {
            if (::fcntl(fd, F_SETFD, FD_CLOEXEC) < 0) {
                const int error = errno;
                close_read();
                close_write();
                throw errno_error("configure pipe", "", error);
            }
        }
    }
    ~Pipe() {
        close_read();
        close_write();
    }
    Pipe(const Pipe&) = delete;
    Pipe& operator=(const Pipe&) = delete;
    Pipe(Pipe&&) = delete;
    Pipe& operator=(Pipe&&) = delete;

    [[nodiscard]] int read_end() const { return fds_[0]; }
    [[nodiscard]] int write_end() const { return fds_[1]; }
    void close_read() { close(0); }
    void close_write() { close(1); }

  private:
    void close(size_t index) {
        if (fds_.at(index) >= 0) {
            ::close(fds_.at(index));
            fds_.at(index) = -1;
        }
    }
    std::array<int, 2> fds_{-1, -1};
};

class SpawnActions {
  public:
    SpawnActions() { check_posix(::posix_spawn_file_actions_init(&actions_), "initialize spawn"); }
    ~SpawnActions() { ::posix_spawn_file_actions_destroy(&actions_); }
    SpawnActions(const SpawnActions&) = delete;
    SpawnActions& operator=(const SpawnActions&) = delete;
    SpawnActions(SpawnActions&&) = delete;
    SpawnActions& operator=(SpawnActions&&) = delete;

    void redirect(int from, int to) {
        check_posix(::posix_spawn_file_actions_adddup2(&actions_, from, to), "redirect child fd");
    }
    void chdir(const std::filesystem::path& directory) {
        check_posix(::posix_spawn_file_actions_addchdir_np(&actions_, directory.c_str()),
                    "set child directory");
    }
    [[nodiscard]] const posix_spawn_file_actions_t* get() const { return &actions_; }

  private:
    posix_spawn_file_actions_t actions_{};
};

// Input is already buffered by the caller. An unlinked temporary file lets the
// child read at its own pace, without pipe backpressure or SIGPIPE when it exits
// early. Only explicit SCM operations and hooks use this process runner.
detail::Fd input_file(const std::string& input) {
    if (input.empty()) {
        return detail::Fd::open("/dev/null", O_RDONLY, 0, "open child stdin");
    }
    std::string name = (std::filesystem::temp_directory_path() / "hz-stdin-XXXXXX").string();
    detail::Fd fd(::mkstemp(name.data()));
    if (fd.get() < 0) {
        throw errno_error("create child stdin", name);
    }
    if (::unlink(name.c_str()) != 0 || ::fcntl(fd.get(), F_SETFD, FD_CLOEXEC) < 0) {
        throw errno_error("configure child stdin", name);
    }
    size_t written = 0;
    while (written < input.size()) {
        const auto count = ::write(fd.get(), input.data() + written, input.size() - written);
        if (count < 0 && errno == EINTR) {
            continue;
        }
        if (count <= 0) {
            throw errno_error("write child stdin", name);
        }
        written += static_cast<size_t>(count);
    }
    if (::lseek(fd.get(), 0, SEEK_SET) < 0) {
        throw errno_error("rewind child stdin", name);
    }
    return fd;
}

std::vector<std::string>
environment_with(const std::vector<std::pair<std::string, std::string>>& overrides) {
    std::map<std::string, std::string> variables;
    for (char** entry = environ; *entry != nullptr; ++entry) {
        std::string_view text(*entry);
        auto equals = text.find('=');
        if (equals != std::string_view::npos) {
            variables[std::string(text.substr(0, equals))] = std::string(text.substr(equals + 1));
        }
    }
    for (const auto& [name, value] : overrides) {
        variables[name] = value;
    }
    std::vector<std::string> result;
    result.reserve(variables.size());
    for (const auto& [name, value] : variables) {
        result.push_back(std::format("{}={}", name, value));
    }
    return result;
}

std::vector<char*> pointers(std::vector<std::string>& strings) {
    std::vector<char*> result;
    result.reserve(strings.size() + 1);
    for (auto& text : strings) {
        result.push_back(text.data());
    }
    result.push_back(nullptr);
    return result;
}

void read_output(Pipe& pipe, std::string& output) {
    std::array<char, 1 << 16> buffer{};
    const auto count = ::read(pipe.read_end(), buffer.data(), buffer.size());
    if (count > 0) {
        output.append(buffer.data(), static_cast<size_t>(count));
    } else if (count == 0) {
        pipe.close_read();
    } else if (errno != EINTR) {
        throw errno_error("read child output", "");
    }
}

void capture(Pipe& out, Pipe& err, ProcessResult& result) {
    while (out.read_end() >= 0 || err.read_end() >= 0) {
        std::array<pollfd, 2> fds{{{.fd = out.read_end(), .events = POLLIN, .revents = 0},
                                   {.fd = err.read_end(), .events = POLLIN, .revents = 0}}};
        if (::poll(fds.data(), fds.size(), -1) < 0) {
            if (errno == EINTR) {
                continue;
            }
            throw errno_error("wait for child output", "");
        }
        if (fds[0].revents != 0) {
            read_output(out, result.out);
        }
        if (fds[1].revents != 0) {
            read_output(err, result.err);
        }
    }
}

int wait_for(pid_t pid) {
    int status = 0;
    while (::waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) {
            throw errno_error("wait for child", "");
        }
    }
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}

} // namespace

ProcessResult run_process(const std::vector<std::string>& argv, const ProcessOptions& options) {
    if (argv.empty() || argv.front().empty()) {
        throw Error(ErrorKind::invalid_argument, "a process needs a non-empty executable name");
    }
    const auto in = input_file(options.input);
    Pipe out;
    Pipe err;
    SpawnActions actions;
    actions.redirect(in.get(), STDIN_FILENO);
    if (options.passthrough) {
        actions.redirect(STDERR_FILENO, STDOUT_FILENO);
    } else {
        actions.redirect(out.write_end(), STDOUT_FILENO);
        actions.redirect(err.write_end(), STDERR_FILENO);
    }
    if (!options.cwd.empty()) {
        actions.chdir(options.cwd);
    }

    std::vector<std::string> arguments = argv;
    std::vector<std::string> environment = environment_with(options.env);
    auto argument_pointers = pointers(arguments);
    auto environment_pointers = pointers(environment);
    pid_t pid = 0;
    check_posix(::posix_spawnp(&pid, arguments.front().c_str(), actions.get(), nullptr,
                               argument_pointers.data(), environment_pointers.data()),
                std::format("cannot run {}", arguments.front()));
    out.close_write();
    err.close_write();

    ProcessResult result;
    try {
        if (!options.passthrough) {
            capture(out, err, result);
        }
    } catch (...) {
        ::kill(pid, SIGKILL);
        int ignored = 0;
        while (::waitpid(pid, &ignored, 0) < 0 && errno == EINTR) {
        }
        throw;
    }
    result.exit_code = wait_for(pid);
    return result;
}

} // namespace hz
