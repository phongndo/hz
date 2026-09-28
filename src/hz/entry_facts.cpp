#include "hz/entry_facts.hpp"

#ifdef __APPLE__

#include "hz/error.hpp"
#include "hz/metadata.hpp"

#include <cerrno>
#include <cstddef>
#include <cstring>
#include <sys/acl.h> // before kauth.h, which then declares the ACL structures
#include <sys/attr.h>
#include <sys/kauth.h>
#include <sys/vnode.h>
#include <unistd.h>

namespace hz {

namespace {

// Room for many entries, or one entry with a large ACL.
constexpr std::size_t facts_buffer_size = std::size_t{64} * 1024;

// Attributes are packed in the order of their bits, after the returned set
// and any error; parse_facts reads them in this order.
static_assert(ATTR_CMN_NAME < ATTR_CMN_DEVID && ATTR_CMN_DEVID < ATTR_CMN_OBJTYPE &&
              ATTR_CMN_OBJTYPE < ATTR_CMN_MODTIME && ATTR_CMN_MODTIME < ATTR_CMN_CHGTIME &&
              ATTR_CMN_CHGTIME < ATTR_CMN_ACCTIME && ATTR_CMN_ACCTIME < ATTR_CMN_OWNERID &&
              ATTR_CMN_OWNERID < ATTR_CMN_GRPID && ATTR_CMN_GRPID < ATTR_CMN_ACCESSMASK &&
              ATTR_CMN_ACCESSMASK < ATTR_CMN_EXTENDED_SECURITY &&
              ATTR_CMN_EXTENDED_SECURITY < ATTR_CMN_FILEID);
static_assert(ATTR_FILE_LINKCOUNT < ATTR_FILE_DATALENGTH);

// A listing asking about ACLs gets each entry's extended flags instead of
// its security data, which costs APFS several times more per entry to read.
// An ACL is stored as an extended attribute, and few entries have any, so
// only those are asked about their ACL one by one.
attrlist facts_request(bool bulk, bool acl) {
    attrlist request{};
    request.bitmapcount = ATTR_BIT_MAP_COUNT;
    request.commonattr = ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME | ATTR_CMN_DEVID |
                         ATTR_CMN_OBJTYPE | ATTR_CMN_MODTIME | ATTR_CMN_CHGTIME | ATTR_CMN_ACCTIME |
                         ATTR_CMN_OWNERID | ATTR_CMN_GRPID | ATTR_CMN_ACCESSMASK | ATTR_CMN_FILEID;
    if (acl && !bulk) {
        request.commonattr |= ATTR_CMN_EXTENDED_SECURITY;
    }
    if (acl && bulk) {
        request.forkattr = ATTR_CMNEXT_EXT_FLAGS; // needs FSOPT_ATTR_CMN_EXTENDED
    }
    if (bulk) {
        request.commonattr |= ATTR_CMN_ERROR;
    }
    request.fileattr = ATTR_FILE_LINKCOUNT | ATTR_FILE_DATALENGTH;
    return request;
}

template <typename T> T take(const char*& field) {
    T value{};
    std::memcpy(&value, field, sizeof value);
    field += sizeof value;
    return value;
}

// Whether the security attribute at `reference` holds any ACL entries.
bool holds_acl(const char* reference, const attrreference_t& value) {
    constexpr auto count_offset =
        offsetof(kauth_filesec, fsec_acl) + offsetof(kauth_acl, acl_entrycount);
    if (value.attr_length == 0) {
        return false;
    }
    if (value.attr_length < count_offset + sizeof(std::uint32_t)) {
        return true; // unreadable: assume the worst
    }
    std::uint32_t count = 0;
    std::memcpy(&count, reference + value.attr_dataoffset + count_offset, sizeof count);
    return count != 0 && count != KAUTH_FILESEC_NOACL;
}

unsigned char type_of(fsobj_type_t type) {
    switch (type) {
    case VREG:
        return DT_REG;
    case VDIR:
        return DT_DIR;
    case VLNK:
        return DT_LNK;
    default:
        return DT_FIFO;
    }
}

// Whether the entry `name` of the open directory `directory` has an ACL.
bool entry_has_acl(int directory, const std::string& name, const std::filesystem::path& path) {
    attrlist request{};
    request.bitmapcount = ATTR_BIT_MAP_COUNT;
    request.commonattr = ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_EXTENDED_SECURITY;
    std::vector<char> buffer(facts_buffer_size);
    if (::getattrlistat(directory, name.c_str(), &request, buffer.data(), buffer.size(),
                        FSOPT_NOFOLLOW) != 0) {
        throw errno_error("read attributes", path / name);
    }
    const char* field = buffer.data() + sizeof(std::uint32_t);
    const auto returned = take<attribute_set_t>(field);
    if ((returned.commonattr & ATTR_CMN_EXTENDED_SECURITY) == 0) {
        return false;
    }
    const char* reference = field;
    return holds_acl(reference, take<attrreference_t>(field));
}

// Parses one packed attribute group, which starts with its length. Sets
// `may_have_acl` unless the entry's extended flags rule out an ACL.
EntryFacts parse_facts(const char* group, const std::filesystem::path& directory,
                       bool& may_have_acl) {
    may_have_acl = true;
    const char* field = group + sizeof(std::uint32_t);
    const auto returned = take<attribute_set_t>(field);
    const auto has = [&](attrgroup_t bit) { return (returned.commonattr & bit) != 0; };
    EntryFacts facts;
    std::uint32_t error = 0;
    if (has(ATTR_CMN_ERROR)) {
        error = take<std::uint32_t>(field);
    }
    if (has(ATTR_CMN_NAME)) {
        const char* reference = field;
        facts.name = reference + take<attrreference_t>(field).attr_dataoffset;
    }
    if (error != 0) {
        throw errno_error("read attributes", directory / facts.name, static_cast<int>(error));
    }
    constexpr attrgroup_t needed = ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_MODTIME |
                                   ATTR_CMN_CHGTIME | ATTR_CMN_ACCTIME | ATTR_CMN_OWNERID |
                                   ATTR_CMN_GRPID | ATTR_CMN_ACCESSMASK;
    if ((returned.commonattr & needed) != needed) {
        return facts;
    }
    facts.device = take<dev_t>(field);
    const unsigned char type = type_of(take<fsobj_type_t>(field));
    facts.times[1] = take<timespec>(field);
    facts.change = take<timespec>(field);
    facts.times[0] = take<timespec>(field);
    facts.uid = take<uid_t>(field);
    facts.gid = take<gid_t>(field);
    facts.mode = static_cast<mode_t>(take<std::uint32_t>(field) & permission_bits);
    if (has(ATTR_CMN_EXTENDED_SECURITY)) {
        const char* reference = field;
        facts.acl = holds_acl(reference, take<attrreference_t>(field));
    }
    if (!has(ATTR_CMN_FILEID)) {
        return facts;
    }
    facts.inode = static_cast<ino_t>(take<std::uint64_t>(field));
    if (type != DT_DIR) {
        if ((returned.fileattr & ATTR_FILE_LINKCOUNT) != 0) {
            facts.links = take<std::uint32_t>(field);
        } else if (type == DT_REG) {
            return facts; // a hard link count is needed to copy one
        }
        if ((returned.fileattr & ATTR_FILE_DATALENGTH) != 0) {
            facts.size = take<off_t>(field);
        }
    }
    // Extended common attributes follow the file attributes.
    if ((returned.forkattr & ATTR_CMNEXT_EXT_FLAGS) != 0) {
        may_have_acl = (take<std::uint64_t>(field) & EF_NO_XATTRS) == 0;
    }
    facts.type = type;
    return facts;
}

} // namespace

struct stat EntryFacts::as_stat() const {
    struct stat info{};
    info.st_dev = device;
    info.st_ino = inode;
    info.st_uid = uid;
    info.st_gid = gid;
    info.st_nlink = static_cast<nlink_t>(links);
    info.st_size = size.value_or(0);
    info.st_atimespec = times[0];
    info.st_mtimespec = times[1];
    info.st_ctimespec = change;
    mode_t kind = 0;
    switch (type) {
    case DT_REG:
        kind = S_IFREG;
        break;
    case DT_DIR:
        kind = S_IFDIR;
        break;
    case DT_LNK:
        kind = S_IFLNK;
        break;
    default:
        break;
    }
    info.st_mode = static_cast<mode_t>(kind | mode);
    return info;
}

EntryFacts entry_facts(int directory, const std::filesystem::path& path) {
    attrlist request = facts_request(false, true);
    std::vector<char> buffer(facts_buffer_size);
    if (::fgetattrlist(directory, &request, buffer.data(), buffer.size(), 0) != 0) {
        throw errno_error("read attributes", path);
    }
    bool may_have_acl = false; // this request reads the ACL itself
    auto facts = parse_facts(buffer.data(), path.parent_path(), may_have_acl);
    facts.name = path.filename();
    return facts;
}

std::vector<EntryFacts> list_entry_facts(int directory, const std::filesystem::path& path,
                                         bool acl) {
    attrlist request = facts_request(true, acl);
    std::vector<char> buffer(facts_buffer_size);
    std::vector<EntryFacts> entries;
    // Reads from the start: the directory may already have been listed.
    if (::lseek(directory, 0, SEEK_SET) != 0) {
        throw errno_error("rewind directory", path);
    }
    for (;;) {
        const int count = ::getattrlistbulk(directory, &request, buffer.data(), buffer.size(),
                                            acl ? FSOPT_ATTR_CMN_EXTENDED : 0);
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            throw errno_error("list directory", path);
        }
        if (count == 0) {
            return entries;
        }
        const char* group = buffer.data();
        for (int i = 0; i < count; ++i) {
            bool may_have_acl = false;
            entries.push_back(parse_facts(group, path, may_have_acl));
            if (acl && may_have_acl && entries.back().type != DT_UNKNOWN) {
                entries.back().acl = entry_has_acl(directory, entries.back().name, path);
            }
            std::uint32_t length = 0;
            std::memcpy(&length, group, sizeof length);
            group += length;
        }
    }
}

} // namespace hz

#endif
