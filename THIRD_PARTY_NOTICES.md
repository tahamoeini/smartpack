# Third-party software notices

SmartPack uses Rust crates and JavaScript packages listed in the checked-in lockfiles. The target-specific release workflows generate a separate bundled notice file for the dependencies present in each native build, including their upstream license and notice texts. To generate notices for a local build, first fetch Rust dependencies and install the desktop packages, then run `tools/generate_third_party_notices.py` as described in the README.

The Apache License 2.0 text in [LICENSE](LICENSE) applies to SmartPack-authored code. It does not replace third-party terms. The notice generator fails the release build if a resolved target dependency has no license/notice text file.
