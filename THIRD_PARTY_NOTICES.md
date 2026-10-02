# Third-party software notices

SmartPack depends on Rust and JavaScript packages listed in the checked-in lockfiles. The resolved Rust package metadata includes MIT, Apache-2.0, BSD, Zlib, Unicode-3.0, and MPL-2.0 licenses, plus packages that offer multiple license alternatives. Review each selected package and its applicable license before distribution; SmartPack's license does not replace third-party terms.

The Apache License 2.0 text in [LICENSE](LICENSE) applies to SmartPack-authored code. It does not replace third-party notices. Before distributing binaries, generate and review a complete direct and transitive dependency notice bundle for `rust_crates/Cargo.lock`, `desktop/src-tauri/Cargo.lock`, and `desktop/package-lock.json`; include every required upstream license text and attribution, and archive the generated notice with the release. The full notice bundle is not yet included.
