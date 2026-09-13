/* SDK-owned layouts. Native identity acquisition runs only in supervised workers. */
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOKitLib.h>
#include <DiskArbitration/DiskArbitration.h>
#include <IOKit/storage/IOBlockStorageDriver.h>
#include <sys/mount.h>
#include <sys/sysctl.h>
#include <stdint.h>
#include <stddef.h>
#include <errno.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define OUTPUT_LIMIT (4U * 1024U * 1024U)
#define RECORD_LIMIT 4096

/* libc intentionally keeps fsid_t's members private in Rust. Read them using
 * the SDK field names rather than guessing a Rust representation. */
void storage_fsid_values(const fsid_t *fsid, int32_t output[2]) {
    output[0] = fsid->val[0]; output[1] = fsid->val[1];
}

static void put_uint(CFMutableDictionaryRef dict, CFStringRef key, uint64_t n) {
    char text[32];
    snprintf(text, sizeof(text), "%" PRIu64, n);
    CFStringRef value = CFStringCreateWithCString(NULL, text, kCFStringEncodingASCII);
    CFDictionarySetValue(dict, key, value);
    CFRelease(value);
}

/* Inspect only each driver's direct IOService children and their own properties. */
static void direct_media(io_registry_entry_t driver, CFMutableDictionaryRef row, unsigned *records) {
    io_iterator_t children = IO_OBJECT_NULL;
    CFMutableArrayRef candidates = CFArrayCreateMutable(NULL, 0, &kCFTypeArrayCallBacks);
    bool ok = candidates && IORegistryEntryGetChildIterator(driver, kIOServicePlane, &children) == KERN_SUCCESS;
    io_object_t child;
    while (ok && (child = IOIteratorNext(children)) != IO_OBJECT_NULL) {
        if (++*records > RECORD_LIMIT) { IOObjectRelease(child); ok = false; break; }
        if (IOObjectConformsTo(child, "IOMedia")) {
            CFTypeRef whole = IORegistryEntryCreateCFProperty(child, CFSTR("Whole"), NULL, 0);
            CFTypeRef bsd = IORegistryEntryCreateCFProperty(child, CFSTR("BSD Name"), NULL, 0);
            uint64_t registry_id;
            if (!whole || CFGetTypeID(whole) != CFBooleanGetTypeID() ||
                !bsd || CFGetTypeID(bsd) != CFStringGetTypeID() ||
                IORegistryEntryGetRegistryEntryID(child, &registry_id) != KERN_SUCCESS) ok = false;
            else {
                CFMutableDictionaryRef media = CFDictionaryCreateMutable(NULL, 0,
                    &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
                CFDictionarySetValue(media, CFSTR("whole"), whole);
                CFDictionarySetValue(media, CFSTR("bsd_name"), bsd);
                put_uint(media, CFSTR("registry_entry_id"), registry_id);
                CFArrayAppendValue(candidates, media); CFRelease(media);
            }
            if (whole) CFRelease(whole);
            if (bsd) CFRelease(bsd);
        }
        IOObjectRelease(child);
    }
    if (children) IOObjectRelease(children);
    CFDictionarySetValue(row, CFSTR("media_mapping_state"), ok ? CFSTR("ok") : CFSTR("unavailable"));
    if (candidates) { CFDictionarySetValue(row, CFSTR("whole_media_candidates"), candidates); CFRelease(candidates); }
}

static int export_plist(CFPropertyListRef value, unsigned char **out, size_t *length) {
    *out = NULL; *length = 0;
    CFErrorRef error = NULL;
    CFDataRef data = CFPropertyListCreateData(NULL, value, kCFPropertyListBinaryFormat_v1_0, 0, &error);
    if (error) CFRelease(error);
    if (!data) return EIO;
    CFIndex size = CFDataGetLength(data);
    int result = 0;
    if (size <= 0 || (uint64_t)size > OUTPUT_LIMIT) result = EOVERFLOW;
    else if (!(*out = malloc((size_t)size))) result = ENOMEM;
    else { memcpy(*out, CFDataGetBytePtr(data), (size_t)size); *length = (size_t)size; }
    CFRelease(data);
    return result;
}

/* Caller verifies this exact local mount's fsid/source both before and after. */
int storage_mount_identity(const char *path, unsigned char **out, size_t *length) {
    *out = NULL; *length = 0;
    DASessionRef session = DASessionCreate(NULL);
    if (!session) return EIO;
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)path, strlen(path), true);
    DADiskRef disk = url ? DADiskCreateFromVolumePath(NULL, session, url) : NULL;
    if (url) CFRelease(url);
    if (!disk) { CFRelease(session); return ENOENT; }
    CFDictionaryRef description = DADiskCopyDescription(disk);
    CFMutableDictionaryRef row = CFDictionaryCreateMutable(NULL, 0,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (description) {
        CFTypeRef uuid = CFDictionaryGetValue(description, kDADiskDescriptionVolumeUUIDKey);
        if (uuid && CFGetTypeID(uuid) == CFUUIDGetTypeID()) {
            CFStringRef text = CFUUIDCreateString(NULL, uuid);
            if (text) { CFDictionarySetValue(row, CFSTR("volume_uuid"), text); CFRelease(text); }
        }
        CFTypeRef bsd = CFDictionaryGetValue(description, kDADiskDescriptionMediaBSDNameKey);
        if (bsd && CFGetTypeID(bsd) == CFStringGetTypeID()) CFDictionarySetValue(row, CFSTR("media_bsd_name"), bsd);
        CFRelease(description);
    }
    io_service_t media = DADiskCopyIOMedia(disk);
    if (media) {
        uint64_t registry_id;
        if (IORegistryEntryGetRegistryEntryID(media, &registry_id) == KERN_SUCCESS)
            put_uint(row, CFSTR("media_registry_id"), registry_id);
        // A boot mount may be an AppleAPFSSnapshot. Its direct IOService parent
        // is authoritative volume ancestry, unlike a guessed BSD suffix.
        if (IOObjectConformsTo(media, "AppleAPFSSnapshot")) {
            io_registry_entry_t parent = IO_OBJECT_NULL;
            if (IORegistryEntryGetParentEntry(media, kIOServicePlane, &parent) == KERN_SUCCESS) {
                if (IOObjectConformsTo(parent, "AppleAPFSVolume")) {
                    CFTypeRef uuid = IORegistryEntryCreateCFProperty(parent, CFSTR("UUID"), NULL, 0);
                    CFTypeRef bsd = IORegistryEntryCreateCFProperty(parent, CFSTR("BSD Name"), NULL, 0);
                    uint64_t parent_id;
                    if (uuid && CFGetTypeID(uuid) == CFStringGetTypeID() && bsd &&
                        CFGetTypeID(bsd) == CFStringGetTypeID() &&
                        IORegistryEntryGetRegistryEntryID(parent, &parent_id) == KERN_SUCCESS) {
                        CFDictionarySetValue(row, CFSTR("parent_volume_uuid"), uuid);
                        CFDictionarySetValue(row, CFSTR("parent_media_bsd_name"), bsd);
                        put_uint(row, CFSTR("parent_media_registry_id"), parent_id);
                    }
                    if (uuid) CFRelease(uuid);
                    if (bsd) CFRelease(bsd);
                }
                IOObjectRelease(parent);
            }
        }
        IOObjectRelease(media);
    }
    int result = export_plist(row, out, length);
    CFRelease(row); CFRelease(disk); CFRelease(session);
    return result;
}

/* The returned malloc buffer is owned by Rust and released via storage_native_free. */
int storage_iokit(unsigned char **out, size_t *length) {
    *out = NULL; *length = 0;
    io_iterator_t iterator = IO_OBJECT_NULL;
    kern_return_t result = IOServiceGetMatchingServices(kIOMainPortDefault,
        IOServiceMatching("IOBlockStorageDriver"), &iterator);
    if (result != KERN_SUCCESS) return EIO;
    CFMutableArrayRef rows = CFArrayCreateMutable(NULL, 0, &kCFTypeArrayCallBacks);
    if (!rows) { IOObjectRelease(iterator); return ENOMEM; }
    io_object_t entry;
    int error = 0;
    unsigned media_records = 0;
    while ((entry = IOIteratorNext(iterator)) != IO_OBJECT_NULL) {
        if (CFArrayGetCount(rows) >= RECORD_LIMIT) { IOObjectRelease(entry); error = EOVERFLOW; break; }
        uint64_t registry_id;
        if (IORegistryEntryGetRegistryEntryID(entry, &registry_id) != KERN_SUCCESS) {
            IOObjectRelease(entry); error = EIO; break;
        }
        CFMutableDictionaryRef row = CFDictionaryCreateMutable(NULL, 0,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        put_uint(row, CFSTR("registry_id"), registry_id);
        direct_media(entry, row, &media_records);
        CFTypeRef stats = IORegistryEntryCreateCFProperty(entry, CFSTR(kIOBlockStorageDriverStatisticsKey), NULL, 0);
        CFMutableDictionaryRef normalized = CFDictionaryCreateMutable(NULL, 0,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        const CFStringRef keys[] = {
            CFSTR(kIOBlockStorageDriverStatisticsBytesReadKey), CFSTR(kIOBlockStorageDriverStatisticsBytesWrittenKey),
            CFSTR(kIOBlockStorageDriverStatisticsReadsKey), CFSTR(kIOBlockStorageDriverStatisticsWritesKey),
            CFSTR(kIOBlockStorageDriverStatisticsReadErrorsKey), CFSTR(kIOBlockStorageDriverStatisticsWriteErrorsKey),
            CFSTR(kIOBlockStorageDriverStatisticsReadRetriesKey), CFSTR(kIOBlockStorageDriverStatisticsWriteRetriesKey),
            CFSTR(kIOBlockStorageDriverStatisticsTotalReadTimeKey), CFSTR(kIOBlockStorageDriverStatisticsTotalWriteTimeKey)
        };
        if (stats && CFGetTypeID(stats) == CFDictionaryGetTypeID()) {
            for (size_t i = 0; i < sizeof(keys) / sizeof(keys[0]); ++i) {
                CFTypeRef value = CFDictionaryGetValue((CFDictionaryRef)stats, keys[i]);
                if (value && CFGetTypeID(value) == CFNumberGetTypeID() && !CFNumberIsFloatType(value)) {
                    int64_t bits;
                    if (CFNumberGetValue(value, kCFNumberSInt64Type, &bits)) put_uint(normalized, keys[i], (uint64_t)bits);
                }
            }
        }
        CFDictionarySetValue(row, CFSTR("statistics"), normalized);
        CFArrayAppendValue(rows, row);
        CFRelease(normalized); CFRelease(row);
        if (stats) CFRelease(stats);
        IOObjectRelease(entry);
    }
    IOObjectRelease(iterator);
    if (!error) {
        CFErrorRef cferror = NULL;
        CFDataRef data = CFPropertyListCreateData(NULL, rows, kCFPropertyListBinaryFormat_v1_0, 0, &cferror);
        if (!data) error = EIO;
        else {
            CFIndex size = CFDataGetLength(data);
            if (size < 0 || (uint64_t)size > OUTPUT_LIMIT) error = EOVERFLOW;
            else {
                *out = malloc((size_t)size);
                if (!*out) error = ENOMEM;
                else { memcpy(*out, CFDataGetBytePtr(data), (size_t)size); *length = (size_t)size; }
            }
            CFRelease(data);
        }
        if (cferror) CFRelease(cferror);
    }
    CFRelease(rows);
    return error;
}

void storage_native_free(unsigned char *buffer) { free(buffer); }

/* vfs.generic.ctlbyfsid + VFS_CTL_NSTATUS is the public dispatch for this query.
 * See Apple NFS/kext/nfs_vfsops.c and xnu/bsd/vfs/vfs_subr.c. Never copy their ABI.
 * malloc supplies alignment; validate flexible-array length even though IDs are omitted. */
int storage_nstatus(int32_t fsid0, int32_t fsid1, char *output, size_t capacity) {
    int mib[CTL_MAXNAME];
    size_t mib_length = CTL_MAXNAME - 1;
    if (sysctlnametomib("vfs.generic.ctlbyfsid", mib, &mib_length) == -1) return errno;
    if (mib_length >= CTL_MAXNAME) return EOVERFLOW;
    mib[mib_length++] = VFS_CTL_NSTATUS;
    struct vfsidctl request;
    memset(&request, 0, sizeof(request));
    request.vc_vers = VFS_CTL_VERS1;
    request.vc_fsid.val[0] = fsid0; request.vc_fsid.val[1] = fsid1;
    for (int attempt = 0; attempt < 3; ++attempt) {
        size_t size = 0;
        if (sysctl(mib, (u_int)mib_length, NULL, &size, &request, sizeof(request)) == -1) return errno;
        if (size < sizeof(struct netfs_status) || size > OUTPUT_LIMIT) return EOVERFLOW;
        struct netfs_status *status = calloc(1, size);
        if (!status) return ENOMEM;
        size_t actual = size;
        if (sysctl(mib, (u_int)mib_length, status, &actual, &request, sizeof(request)) == -1) {
            int error = errno; free(status);
            if (error == ERANGE || error == ENOMEM) continue;
            return error;
        }
        const size_t base = offsetof(struct netfs_status, ns_threadids);
        if (actual > size || actual < sizeof(struct netfs_status) || actual < base ||
            status->ns_threadcount > (actual - base) / sizeof(status->ns_threadids[0])) {
            free(status); return EPROTO;
        }
        int written = snprintf(output, capacity,
            "{\"status\":\"ok\",\"not_responding\":%s,\"dead\":%s,\"outstanding_request_entries\":\"%" PRIu32 "\",\"oldest_request_age_seconds\":\"%" PRIu32 "\"}",
            (status->ns_status & VQ_NOTRESP) ? "true" : "false",
            (status->ns_status & VQ_DEAD) ? "true" : "false", status->ns_threadcount, status->ns_waittime);
        free(status);
        if (written < 0 || (size_t)written >= capacity) return EOVERFLOW;
        return 0;
    }
    return EAGAIN;
}
