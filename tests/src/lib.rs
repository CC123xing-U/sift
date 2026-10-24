//! sift-tests —— 工作区级集成测试包（DESIGN §14）。
//!
//! - 常规集成测试：`tests/tests/*.rs`
//! - 需要管理员与真实 NTFS 卷的测试：`tests/tests/windows_integration.rs`，
//!   默认 `#[ignore]`，通过 `cargo test -p sift-tests -- --ignored` 手动运行。
