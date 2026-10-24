//! 协作式取消令牌（DESIGN §2.2-6：每次按键取消上一次搜索）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 取消令牌：长循环定期检查（DESIGN §7.3：约每 4K 条一次）。
///
/// 内部 `Arc<AtomicBool>`：`Clone` 廉价且**共享同一标志**
/// （Orchestrator 冻结签名），可分别交给 UI 线程与搜索线程。
#[derive(Debug, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
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
            cancelled: Arc::clone(&self.cancelled),
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
    fn clone_shares_state() {
        // 冻结签名：内部 Arc<AtomicBool>，Clone 廉价且共享同一标志
        let t = CancelToken::new();
        let c = t.clone();
        assert!(!c.is_cancelled());
        t.cancel();
        assert!(c.is_cancelled()); // 克隆随原令牌一起取消
        c.reset();
        assert!(!t.is_cancelled()); // 反向亦然
    }

    #[test]
    fn cancel_from_thread() {
        let t = CancelToken::new();
        let c = t.clone();
        std::thread::spawn(move || {
            c.cancel();
        })
        .join()
        .unwrap();
        assert!(t.is_cancelled());
    }
}
