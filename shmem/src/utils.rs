use std::{future::Future, task::Poll};

pub(crate) struct Blocker<E: std::error::Error, F: Fn() -> Result<bool, E>> {
    is_ready: F,
}

pub(crate) fn blocker<E: std::error::Error, F: Fn() -> Result<bool, E>>(
    is_ready: F,
) -> Blocker<E, F> {
    Blocker { is_ready }
}

impl<E: std::error::Error, F: Fn() -> Result<bool, E>> Future for Blocker<E, F> {
    type Output = Result<(), E>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let is_ready = (self.is_ready)();

        match is_ready {
            Ok(true) => Poll::Ready(Ok(())),
            Ok(false) => {
                // Schedule a wake-up to be polled again
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}
