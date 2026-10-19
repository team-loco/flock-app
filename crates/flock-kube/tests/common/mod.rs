pub mod kubeconfig;
pub mod scratch;

use std::fmt::Debug;
use std::time::Duration;

pub const WAIT: Duration = Duration::from_secs(600);
pub const POLL: Duration = Duration::from_millis(500);

pub async fn eventually<T, F, Fut>(what: &str, observe: F, done: impl Fn(&T) -> bool) -> T
where
    T: Debug,
    F: Fn() -> Fut,
    Fut: Future<Output = T>,
{
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let observed = observe().await;
        if done(&observed) {
            return observed;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} not met within {WAIT:?}; last observed: {observed:#?}"
        );
        tokio::time::sleep(POLL).await;
    }
}
