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

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        let is_ready = (self.is_ready)();

        if is_ready.is_err() {
            return Poll::Ready(Err(is_ready.err().unwrap()));
        }

        if is_ready.unwrap() {
            return Poll::Ready(Ok(()));
        }

        Poll::Pending
    }
}
