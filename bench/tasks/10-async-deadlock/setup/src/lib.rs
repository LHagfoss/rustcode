use std::sync::Arc;
use tokio::sync::Mutex;

pub struct SharedCounter {
    val_a: Arc<Mutex<i32>>,
    val_b: Arc<Mutex<i32>>,
}

impl SharedCounter {
    pub fn new() -> Self {
        Self {
            val_a: Arc::new(Mutex::new(0)),
            val_b: Arc::new(Mutex::new(0)),
        }
    }

    pub async fn increment_a_then_b(&self) -> i32 {
        let mut guard_a = self.val_a.lock().await;
        *guard_a += 1;
        tokio::task::yield_now().await;
        let mut guard_b = self.val_b.lock().await;
        *guard_b += 1;
        *guard_a + *guard_b
    }

    pub async fn increment_b_then_a(&self) -> i32 {
        let mut guard_b = self.val_b.lock().await;
        *guard_b += 1;
        tokio::task::yield_now().await;
        let mut guard_a = self.val_a.lock().await;
        *guard_a += 1;
        *guard_a + *guard_b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_concurrent_increments() {
        let counter = Arc::new(SharedCounter::new());
        let c1 = counter.clone();
        let c2 = counter.clone();

        let h1 = tokio::spawn(async move { c1.increment_a_then_b().await });
        let h2 = tokio::spawn(async move { c2.increment_b_then_a().await });

        let res1 = h1.await.unwrap();
        let res2 = h2.await.unwrap();

        assert!(res1 > 0 && res2 > 0);
    }
}
