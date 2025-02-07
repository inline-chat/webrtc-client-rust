use core_foundation::{
    base::{CFAllocator, CFRelease},
    dictionary::CFDictionaryRef,
    number::CFNumberRef,
    string::CFStringRef,
};
use std::os::raw::{c_char, c_int, c_void};

#[link(name = "IOKit", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn IOServiceGetMatchingServices(
        masterPort: i32,
        matching: CFDictionaryRef,
        existing: *mut io_iterator_t,
    ) -> i32;
    fn IOServiceMatching(name: *const i8) -> CFDictionaryRef;
    fn IOIteratorNext(iterator: io_iterator_t) -> io_registry_entry_t;
    fn IORegistryEntryCreateCFProperties(
        entry: io_registry_entry_t,
        properties: *mut CFDictionaryRef,
        allocator: CFAllocatorRef,
        options: IOOptionBits,
    ) -> i32;
    fn IOObjectRelease(object: io_object_t);

    fn CFDictionaryGetValue(theDict: CFDictionaryRef, key: *const c_void) -> *const c_void;

    fn CFNumberGetValue(
        number: CFNumberRef,
        theType: CFNumberType,
        valuePtr: *mut c_void,
    ) -> Boolean;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        cStr: *const c_char,
        encoding: CFStringEncoding,
    ) -> CFStringRef;
}

type io_iterator_t = *mut c_void;
type io_registry_entry_t = *mut c_void;
type io_object_t = *mut c_void;
type CFAllocatorRef = *const c_void;
type IOOptionBits = u32;
type Boolean = u8;
type CFNumberType = u32;
type CFStringEncoding = u32;

const KERN_SUCCESS: i32 = 0;
const kIOMasterPortDefault: i32 = 0;
const kCFNumberSInt64Type: CFNumberType = 4;
const kCFStringEncodingUTF8: CFStringEncoding = 0x08000100;

pub fn get_idle_time() -> f64 {
    let mut idle_seconds = -1.0;
    let mut iter: io_iterator_t = std::ptr::null_mut();

    unsafe {
        let matching = IOServiceMatching(b"IOHIDSystem\0".as_ptr() as *const i8);
        if IOServiceGetMatchingServices(kIOMasterPortDefault, matching, &mut iter) == KERN_SUCCESS {
            let entry = IOIteratorNext(iter);
            if !entry.is_null() {
                let mut dict: CFDictionaryRef = std::ptr::null_mut();
                if IORegistryEntryCreateCFProperties(entry, &mut dict, std::ptr::null(), 0)
                    == KERN_SUCCESS
                {
                    let idle_time_key = CFStringCreateWithCString(
                        std::ptr::null(),
                        b"HIDIdleTime\0".as_ptr() as *const c_char,
                        kCFStringEncodingUTF8,
                    );
                    let value = CFDictionaryGetValue(dict, idle_time_key as *const c_void);
                    if !value.is_null() {
                        let number = value as CFNumberRef;
                        let mut nanoseconds: i64 = 0;
                        if CFNumberGetValue(
                            number,
                            kCFNumberSInt64Type,
                            &mut nanoseconds as *mut _ as *mut c_void,
                        ) != 0
                        {
                            idle_seconds = nanoseconds as f64 / 1_000_000_000.0;
                        }
                    }
                    CFRelease(idle_time_key as *const c_void);
                    CFRelease(dict as *mut c_void);
                }
                IOObjectRelease(entry as *mut c_void);
            }
            IOObjectRelease(iter as *mut c_void);
        }
    }

    idle_seconds
}
