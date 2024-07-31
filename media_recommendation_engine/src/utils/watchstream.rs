use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::Stream;
use tokio_util::sync::ReusableBoxFuture;

use tokio::sync::watch::{error::RecvError, Receiver};

async fn make_future<T: Clone + Send + Sync>(
    mut rx: Receiver<T>,
) -> (Result<(), RecvError>, Receiver<T>) {
    let result = rx.changed().await;
    (result, rx)
}

pub struct WatchStream<T> {
    inner: ReusableBoxFuture<'static, (Result<(), RecvError>, Receiver<T>)>,
}

impl<T: 'static + Clone + Send + Sync> WatchStream<T> {
    /// Create a new `WatchStream`.
    pub fn new(rx: Receiver<T>) -> Self {
        Self {
            inner: ReusableBoxFuture::new(async move { (Ok(()), rx) }),
        }
    }
}

impl<T> Unpin for WatchStream<T> {}

impl<T: Clone + 'static + Send + Sync> Stream for WatchStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (result, mut rx) = ready!(self.inner.poll(cx));
        match result {
            Ok(_) => {
                let received = (*rx.borrow_and_update()).clone();
                self.inner.set(make_future(rx));
                Poll::Ready(Some(received))
            }
            Err(_) => {
                self.inner.set(make_future(rx));
                Poll::Ready(None)
            }
        }
    }
}
