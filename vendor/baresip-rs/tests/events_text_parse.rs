use baresip::events::text_from_ptr;

#[test]
fn text_from_ptr_handles_null() {
    let s = text_from_ptr(std::ptr::null());
    assert!(s.is_none());
}

#[test]
fn text_from_ptr_non_utf8_returns_none() {
    let bytes: [u8; 2] = [0xFF, 0x00];
    let ptr = bytes.as_ptr() as *const i8;
    let s = text_from_ptr(ptr);
    assert!(s.is_none());
}

