#pragma once

#include <filesystem>
#include <string>
#include <utility>
#include <vector>

namespace hz {

struct ProcessOptions {
    std::filesystem::path cwd;                            // empty: inherit
    std::string input;                                    // written to stdin
    std::vector<std::pair<std::string, std::string>> env; // added or replaced
    // Send the child's stdout to our stderr and let it share our stderr,
    // instead of capturing both. Used for hooks, whose output is for people
    // and must not corrupt JSON on our stdout.
    bool passthrough = false;
};

struct ProcessResult {
    int exit_code = -1; // 128 + signal number if killed by a signal
    std::string out;
    std::string err;

    [[nodiscard]] bool ok() const { return exit_code == 0; }
};

// Runs `argv[0]` (searched on PATH) and waits for it. Throws Error(io) on
// spawn or I/O failure. Nonzero child exits are returned in ProcessResult.
ProcessResult run_process(const std::vector<std::string>& argv, const ProcessOptions& options = {});

} // namespace hz
