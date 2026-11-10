//! A profile file (imported, hand-written, or kept with backups on a drive).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(p) = flux_core::Profile::from_toml(text) {
        let _ = p.validate();
        let _ = p.validate_in(flux_core::Scope::System);
        if let Ok(id) = flux_core::ProfileId::new("fuzz") {
            let _ = p.kept_as(&id);
            let _ = p.for_destination("x");
        }
    }
    let _ = flux_compat::export::parse_flux(text);
    let _ = flux_compat::export::parse_recovery_copy(text);
});
