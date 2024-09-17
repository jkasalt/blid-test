#![allow(dead_code)]
use axum::http::Response;
use tower::Service;

struct CookieManager<Store, S> {
    inner: S,
    store: Store,
}

impl<Store, S> CookieManager<Store, S> {
    const fn new(store: Store, inner: S) -> Self {
        Self { inner, store }
    }
}

trait CookieStore {
    fn get(&self, key: &str) -> Option<&str>;
    fn set(&mut self, key: &str, val: String) -> bool;
}

impl<Store, S, B> Service<Response<B>> for CookieManager<Store, S>
where
    S: Service<Response<B>>,
    Store: CookieStore,
{
    type Error = S::Error;
    type Future = S::Future;
    type Response = S::Response;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Response<B>) -> Self::Future {
        req.headers();
        todo!()
    }
}
