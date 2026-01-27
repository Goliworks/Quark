use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use hyper::{
    body::{Body, Frame, Incoming},
    service::Service,
    Request, Response,
};
use pin_project_lite::pin_project;

use crate::{server::server_utils::ProxyHandlerBody, utils::get_current_time};

#[derive(Clone)]
pub struct ServerService<S> {
    inner: S,
    last_activity: Arc<AtomicU64>,
}

impl<S> ServerService<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            last_activity: Arc::new(AtomicU64::new(0)),
        }
    }

    fn update_activity(&self) {
        let now = get_current_time();
        self.last_activity.store(now, Ordering::Relaxed);
    }

    pub fn seconds_since_last_activity(&self) -> u64 {
        let now = get_current_time();
        now - self.last_activity.load(Ordering::Relaxed)
    }
}

impl<S> Service<Request<Incoming>> for ServerService<S>
where
    S: Service<Request<Incoming>, Response = Response<ProxyHandlerBody>> + Clone + Send + 'static,
    S::Error: Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<ActivityTrackingBody<ProxyHandlerBody>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        self.update_activity();
        let inner = self.inner.clone();
        let last_activity = Arc::clone(&self.last_activity);

        Box::pin(async move {
            let res = inner.call(req).await?;
            let (parts, body) = res.into_parts();
            let tracking_body = ActivityTrackingBody::new(body, last_activity);
            Ok(Response::from_parts(parts, tracking_body))
        })
    }
}

pin_project! {
    pub struct ActivityTrackingBody<B> {
        #[pin]
        inner: B,
        last_activity: Arc<AtomicU64>,
    }
}

impl<B> ActivityTrackingBody<B> {
    fn new(inner: B, last_activity: Arc<AtomicU64>) -> Self {
        Self {
            inner,
            last_activity,
        }
    }
}

impl<B> Body for ActivityTrackingBody<B>
where
    B: Body,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        match this.inner.poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.is_data() {
                    // Update last activity.
                    let now = get_current_time();
                    this.last_activity.store(now, Ordering::Relaxed);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}
