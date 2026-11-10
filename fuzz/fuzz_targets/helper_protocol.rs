//! A request line to the administrator helper, which runs as root.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(line) = std::str::from_utf8(data) {
        if let Ok(mut req) = flux_service::protocol::parse_request(line) {
            let _ = req.call.method();
            let _ = req.call.profile_id_mut().map(|id| id.len());
        }
    }
});
