#include "hz/git.hpp"

#include "hz/error.hpp"
#include "hz/fsutil.hpp"
#include "hz/marker.hpp"
#include "hz/process.hpp"
#include "hz/ulid.hpp"

#include <array>
#include <cerrno>
#include <fcntl.h>
#include <format>
#include <fstream>
#include <sstream>
#include <sys/stat.h>
#include <unistd.h>
#include <utility>

namespace hz::git {

namespace fs = std::filesystem;

namespace {

constexpr std::string_view exclude_line = "/.hz-workspace";
constexpr std::string_view unborn_base = "hz-unborn-base";

// Files and directories whose presence means Git is in the middle of an
// operation, so a file-by-file copy could capture a half-written state.
constexpr std::array in_progress{
    std::pair{"index.lock", "another Git process is writing the index"},
    std::pair{"HEAD.lock", "another Git process is updating HEAD"},
    std::pair{"MERGE_HEAD", "a merge is in progress"},
    std::pair{"CHERRY_PICK_HEAD", "a cherry-pick is in progress"},
    std::pair{"REVERT_HEAD", "a revert is in progress"},
    std::pair{"BISECT_LOG", "a bisect is in progress"},
    std::pair{"rebase-merge", "a rebase is in progress"},
    std::pair{"rebase-apply", "a rebase or am is in progress"},
    std::pair{"sequencer", "a cherry-pick or revert sequence is in progress"},
};

std::optional<std::string> read_text(const fs::path& path) {
    std::ifstream in(path, std::ios::binary);
    if (!in) {
        return std::nullopt;
    }
    std::stringstream buffer;
    buffer << in.rdbuf();
    return buffer.str();
}

std::string trim(std::string text) {
    while (!text.empty() && (text.back() == '\n' || text.back() == '\r' || text.back() == ' ')) {
        text.pop_back();
    }
    return text;
}

bool is_object_id(std::string_view text) {
    return (text.size() == 40 || text.size() == 64) &&
           text.find_first_not_of("0123456789abcdef") == std::string_view::npos;
}

// Writes `content` to `path` the way Git does: into `path.lock` created
// exclusively, then renamed over the target.
void write_like_git(const fs::path& path, const std::string& content) {
    const fs::path lock = fs::path(path.string() + ".lock");
    const int fd = ::open(lock.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0644);
    if (fd < 0) {
        throw errno_error("lock", path);
    }
    const auto written = ::write(fd, content.data(), content.size());
    ::close(fd);
    if (!std::cmp_equal(written, content.size()) || ::rename(lock.c_str(), path.c_str()) != 0) {
        const int error = errno;
        ::unlink(lock.c_str());
        throw errno_error("write", path, error);
    }
}

// Resolves a ref in a files-backend repository, following symbolic refs.
std::optional<std::string> resolve_ref(const fs::path& git_dir, std::string ref) {
    for (int depth = 0; depth < 5; ++depth) {
        if (auto loose = read_text(git_dir / ref)) {
            std::string value = trim(*loose);
            if (value.starts_with("ref: ")) {
                ref = value.substr(5);
                continue;
            }
            return is_object_id(value) ? std::optional(value) : std::nullopt;
        }
        if (auto packed = read_text(git_dir / "packed-refs")) {
            std::istringstream lines(*packed);
            std::string line;
            while (std::getline(lines, line)) {
                if (line.size() > 41 && !line.starts_with('#') && !line.starts_with('^')) {
                    const auto space = line.find(' ');
                    if (space != std::string::npos && line.substr(space + 1) == ref) {
                        return line.substr(0, space);
                    }
                }
            }
        }
        return std::nullopt;
    }
    return std::nullopt;
}

void ensure_git_exclude(const fs::path& git_dir) {
    const fs::path exclude = git_dir / "info" / "exclude";
    const std::string current = read_text(exclude).value_or("");
    std::istringstream lines(current);
    for (std::string line; std::getline(lines, line);) {
        if (trim(line) == exclude_line) {
            return;
        }
    }
    make_private_directories(exclude.parent_path());
    std::ofstream out(exclude, std::ios::binary | std::ios::app);
    if (!current.empty() && !current.ends_with('\n')) {
        out << '\n';
    }
    out << "# hz workspace identity marker\n" << exclude_line << '\n';
    out.flush();
    if (!out) {
        throw errno_error("write exclude", exclude);
    }
}

void ensure_hg_ignore(const fs::path& hg_dir) {
    const fs::path ignore = hg_dir / "hz-ignore";
    if (!exists_nofollow(ignore)) {
        std::ofstream(ignore, std::ios::binary) << "syntax: rootglob\n" << marker_name << '\n';
    }
    const fs::path hgrc = hg_dir / "hgrc";
    const std::string current = read_text(hgrc).value_or("");
    if (current.find("ignore.hz") == std::string::npos) {
        std::ofstream out(hgrc, std::ios::binary | std::ios::app);
        out << (current.empty() || current.ends_with('\n') ? "" : "\n")
            << "[ui]\nignore.hz = .hg/hz-ignore\n";
    }
}

ProcessResult git(const fs::path& workspace, std::vector<std::string> arguments,
                  ProcessOptions options = {}) {
    arguments.insert(arguments.begin(), {"git", "-C", workspace.string()});
    options.env.emplace_back("GIT_OPTIONAL_LOCKS", "0");
    return run_process(arguments, options);
}

std::string checked(const ProcessResult& result, std::string_view what) {
    if (!result.ok()) {
        throw Error(ErrorKind::io, std::format("{}: {}", what, trim(result.err)));
    }
    return result.out;
}

std::optional<std::string> rev_parse(const fs::path& workspace, const std::string& revision) {
    auto result = git(workspace, {"rev-parse", "--verify", "--quiet", revision + "^{commit}"});
    if (!result.ok()) {
        return std::nullopt;
    }
    return trim(result.out);
}

std::vector<std::string> split_nul(const std::string& text) {
    std::vector<std::string> parts;
    size_t start = 0;
    while (start < text.size()) {
        const auto end = text.find('\0', start);
        parts.push_back(text.substr(start, end - start));
        if (end == std::string::npos) {
            break;
        }
        start = end + 1;
    }
    return parts;
}

void require_repository(const fs::path& workspace) {
    if (!is_repository(workspace)) {
        throw Error(ErrorKind::invalid_argument,
                    std::format("{} is not a Git repository", workspace.string()));
    }
}

// A binary patch from `base` to the working tree of `workspace`, including
// untracked files, which are added to a throwaway copy of the index as
// intent-to-add so `git diff` sees them.
std::string working_tree_patch(const fs::path& workspace, const std::string& base) {
    const std::string untracked =
        checked(git(workspace, {"ls-files", "--others", "--exclude-standard", "-z"}),
                "list untracked files");
    std::vector<std::pair<std::string, std::string>> env;
    fs::path temporary_index;
    if (!untracked.empty()) {
        const fs::path git_dir = workspace / ".git";
        temporary_index = git_dir / std::format("hz-index-{}", generate_ulid());
        if (exists_nofollow(git_dir / "index")) {
            fs::copy_file(git_dir / "index", temporary_index);
        }
        env.emplace_back("GIT_INDEX_FILE", temporary_index.string());
        auto added = git(workspace,
                         {"--literal-pathspecs", "add", "--intent-to-add", "--pathspec-from-file=-",
                          "--pathspec-file-nul"},
                         {.cwd = {}, .input = untracked, .env = env, .passthrough = false});
        if (!added.ok()) {
            fs::remove(temporary_index);
            throw Error(ErrorKind::io, std::format("prepare untracked files: {}", trim(added.err)));
        }
    }
    auto diff =
        git(workspace,
            {"diff", "--binary", "--no-color", "--no-ext-diff", "--no-textconv", "--src-prefix=a/",
             "--dst-prefix=b/", base, "--", std::format(":(exclude,top){}", marker_name)},
            {.cwd = {}, .input = {}, .env = env, .passthrough = false});
    if (!temporary_index.empty()) {
        std::error_code ignored;
        fs::remove(temporary_index, ignored);
    }
    return checked(diff, "create patch");
}

} // namespace

bool is_repository(const fs::path& workspace) {
    struct stat info{};
    return ::lstat((workspace / ".git").c_str(), &info) == 0 && S_ISDIR(info.st_mode);
}

void check_source(const fs::path& workspace) {
    const fs::path dot_git = workspace / ".git";
    struct stat info{};
    if (::lstat(dot_git.c_str(), &info) != 0) {
        return; // not a Git checkout
    }
    if (!S_ISDIR(info.st_mode)) {
        throw Error(ErrorKind::unsafe_source,
                    std::format("{} is a linked Git worktree or submodule (its .git is a "
                                "file); copy the main checkout instead",
                                workspace.string()));
    }
    for (const auto& [name, reason] : in_progress) {
        if (exists_nofollow(dot_git / name)) {
            throw Error(ErrorKind::unsafe_source,
                        std::format("cannot copy {}: {} (.git/{} exists); try again once it "
                                    "finishes",
                                    workspace.string(), reason, name));
        }
    }
    if (exists_nofollow(dot_git / "reftable")) {
        throw Error(ErrorKind::unsafe_source,
                    "reftable Git sources are not supported; detaching their HEAD requires Git");
    }
    auto check_lock = [](const fs::directory_entry& entry) {
        if (entry.path().extension() == ".lock") {
            throw Error(ErrorKind::unsafe_source,
                        std::format("Git lock {} exists; retry after the operation finishes",
                                    entry.path().string()));
        }
    };
    for (const auto& entry : fs::directory_iterator(dot_git)) {
        check_lock(entry);
    }
    if (fs::is_directory(dot_git / "refs")) {
        for (const auto& entry : fs::recursive_directory_iterator(dot_git / "refs")) {
            check_lock(entry);
        }
    }
}

void prepare_root(const fs::path& workspace) {
    if (is_repository(workspace)) {
        ensure_git_exclude(workspace / ".git");
    }
    if (exists_nofollow(workspace / ".hg" / "store")) {
        ensure_hg_ignore(workspace / ".hg");
    }
}

void prepare_child(const fs::path& workspace) {
    if (exists_nofollow(workspace / ".hg" / "store")) {
        ensure_hg_ignore(workspace / ".hg");
    }
    if (!is_repository(workspace)) {
        return;
    }
    const fs::path git_dir = workspace / ".git";
    ensure_git_exclude(git_dir);

    const auto head = resolve_ref(git_dir, "HEAD");
    if (!head) {
        write_like_git(git_dir / unborn_base, "\n");
        return; // unborn branch: nothing to detach from
    }
    fs::remove(git_dir / unborn_base);
    make_private_directories((git_dir / base_ref).parent_path());
    write_like_git(git_dir / base_ref, *head + "\n");
    write_like_git(git_dir / "HEAD", *head + "\n");
}

Status status(const fs::path& workspace) {
    require_repository(workspace);
    Status result;
    const auto porcelain = checked(
        git(workspace, {"status", "--porcelain=v1", "-z", "--untracked-files=all"}), "git status");
    auto fields = split_nul(porcelain);
    for (size_t i = 0; i < fields.size(); ++i) {
        const auto& field = fields[i];
        if (field.size() < 4) {
            continue;
        }
        result.entries.push_back({.code = field.substr(0, 2), .path = field.substr(3)});
        if (field[0] == 'R' || field[0] == 'C') {
            ++i; // the original path of a rename or copy follows
        }
    }
    auto branch = git(workspace, {"symbolic-ref", "--quiet", "--short", "HEAD"});
    if (branch.ok()) {
        result.branch = trim(branch.out);
    }
    result.head = rev_parse(workspace, "HEAD");
    result.base = rev_parse(workspace, std::string(base_ref));
    return result;
}

Handoff handoff(const fs::path& from, const fs::path& to, bool three_way) {
    require_repository(from);
    require_repository(to);
    if (git::status(to).dirty()) {
        throw Error(
            ErrorKind::conflict,
            std::format("{} has uncommitted changes; commit or stash them first", to.string()));
    }
    Handoff result;
    const auto head = rev_parse(from, "HEAD");
    if (exists_nofollow(from / ".git" / unborn_base) || !head) {
        result.base = trim(checked(git(from, {"hash-object", "-w", "-t", "tree", "--stdin"}),
                                   "resolve the empty tree"));
    } else if (auto base = rev_parse(from, std::string(base_ref))) {
        result.base = *base;
    } else {
        result.base = *head;
    }
    const std::string patch = working_tree_patch(from, result.base);
    if (patch.find_first_not_of(" \t\r\n") == std::string::npos) {
        return result;
    }
    std::vector<std::string> apply{"apply", "--binary"};
    if (three_way) {
        apply.emplace_back("--3way");
    }
    auto check = apply;
    check.emplace_back("--check");
    const ProcessOptions with_patch{.cwd = {}, .input = patch, .env = {}, .passthrough = false};
    auto checked_apply = git(to, check, with_patch);
    if (!checked_apply.ok()) {
        throw Error(ErrorKind::conflict,
                    std::format("the changes do not apply cleanly to {}{}:\n{}", to.string(),
                                three_way ? "" : " (try --3way)", trim(checked_apply.err)));
    }
    checked(git(to, apply, with_patch), "apply patch");
    result.changed = true;
    return result;
}

std::string hg_status(const fs::path& workspace) {
    return checked(run_process({"hg", "--cwd", workspace.string(), "status"}), "hg status");
}

} // namespace hz::git
