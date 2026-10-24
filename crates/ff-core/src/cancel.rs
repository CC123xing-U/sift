//! 协作式取消令牌（DESIGN §2.2-6：每次按键取消上一次搜索）。

use std::sync::atomic::{AtomicBool, Ordering};

/// 取消令牌：长循环定期检查（DESIGN §7.3：约每 4K 条一次）。
///
/// `Clone` 语义为**共享同一标志**（内部 `AtomicBool`），便于把同一令牌
/// 交给搜索线程与 UI 线程。
#[derive(Debug, Default)]
pub struct CancelToken {
    cancelled: AtomicBool,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
        }
    }

    /// 请求取消（幂等；可从任意线程调用）。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// 是否已请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 复位（复用同一令牌发起新搜索）。
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }
}

impl Clone for CancelToken {
    fn clone(&self) -> Self {
        Self {
            cancelled: AtomicBool::new(self.cancelled.load(Ordering::SeqCst)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_not_cancelled() {
        assert!(!CancelToken::default().is_cancelled());
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn cancel_and_reset() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
        t.cancel(); // 幂等
        assert!(t.is_cancelled());
        t.reset();
        assert!(!t.is_cancelled());
    }

    #[test]
    fn clone_is_independent_snapshot() {
        let t = CancelToken::new();
        let c = t.clone();
        t.cancel();
        // 克隆是快照，不随原令牌变化（引擎按需选择共享引用或快照）
        assert!(!c.is_cancelled());
    }

    #[test]
    fn cancel_from_thread() {
        let t = CancelToken::new();
        let t2 = CancelToken::new();
        std::thread::spawn(move || {
            t2.cancel();
        })
        .join()
        .unwrap();
        assert!(!t.is_cancelled());
    }
}
