use axum::extract::connect_info::Connected;
use hyper::{body::HttpBody, Body, HeaderMap, Request, Response};
use pin_project::pin_project;
use std::{future::Future, marker::PhantomData, pin::Pin, task::Poll};
use tower::Service;

pub use axum::extract::connect_info::ConnectInfo;

#[derive(Clone, Debug)]
pub struct HybridMakeServiceWithConnInfo<MakeWeb, Grpc, ConnInfo> {
    make_web: MakeWeb,
    grpc: Grpc,
    _connect_info: PhantomData<fn() -> ConnInfo>,
}

impl<MakeWeb, Grpc, ConnInfo> HybridMakeServiceWithConnInfo<MakeWeb, Grpc, ConnInfo> {
    pub fn new(make_web: MakeWeb, grpc: Grpc) -> Self {
        Self {
            make_web,
            grpc,
            _connect_info: PhantomData,
        }
    }
}

impl<MakeWeb, Grpc, ConnInfo, T> Service<T>
    for HybridMakeServiceWithConnInfo<MakeWeb, Grpc, ConnInfo>
where
    MakeWeb: Service<T>,
    Grpc: Clone,
    T: Clone,
    ConnInfo: Connected<T>,
{
    type Response = HybridServiceWithConnInfo<MakeWeb::Response, Grpc, ConnInfo>;

    type Error = MakeWeb::Error;

    type Future = HybridMakeServiceWithConnInfoFuture<MakeWeb::Future, Grpc, ConnInfo>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.make_web.poll_ready(cx)
    }

    fn call(&mut self, target: T) -> Self::Future {
        let conn_info = ConnInfo::connect_info(target.clone());
        HybridMakeServiceWithConnInfoFuture {
            web_future: self.make_web.call(target),
            grpc: Some(self.grpc.clone()),
            conn_info: Some(conn_info),
        }
    }
}

#[pin_project]
pub struct HybridMakeServiceWithConnInfoFuture<WebFuture, Grpc, ConnInfo> {
    #[pin]
    web_future: WebFuture,
    grpc: Option<Grpc>,
    conn_info: Option<ConnInfo>,
}

impl<WebFuture, Web, WebError, Grpc, ConnInfo> Future
    for HybridMakeServiceWithConnInfoFuture<WebFuture, Grpc, ConnInfo>
where
    WebFuture: Future<Output = Result<Web, WebError>>,
{
    type Output = Result<HybridServiceWithConnInfo<Web, Grpc, ConnInfo>, WebError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context) -> Poll<Self::Output> {
        let this = self.project();
        match this.web_future.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(web)) => Poll::Ready(Ok(HybridServiceWithConnInfo {
                web,
                grpc: this.grpc.take().expect("Cannot poll twice!"),
                conn_info: this.conn_info.take().expect("Cannot poll twice!"),
            })),
        }
    }
}

pub struct HybridServiceWithConnInfo<Web, Grpc, T> {
    web: Web,
    grpc: Grpc,
    conn_info: T,
}

impl<Web, Grpc, WebBody, GrpcBody, T> Service<Request<Body>>
    for HybridServiceWithConnInfo<Web, Grpc, T>
where
    Web: Service<Request<Body>, Response = Response<WebBody>>,
    Grpc: Service<Request<Body>, Response = Response<GrpcBody>>,
    Web::Error: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    Grpc::Error: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    T: Clone + Send + Sync + 'static,
{
    type Response = Response<HybridBody<WebBody, GrpcBody>>;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
    type Future = HybridFuture<Web::Future, Grpc::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        match self.web.poll_ready(cx) {
            Poll::Ready(Ok(())) => match self.grpc.poll_ready(cx) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(e)) => Poll::Ready(Err(e.into())),
                Poll::Pending => Poll::Pending,
            },
            Poll::Ready(Err(e)) => Poll::Ready(Err(e.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        req.extensions_mut()
            .insert(ConnectInfo(self.conn_info.clone()));
        if req.headers().get("content-type").map(|x| x.as_bytes()) == Some(b"application/grpc") {
            HybridFuture::Grpc(self.grpc.call(req))
        } else {
            HybridFuture::Web(self.web.call(req))
        }
    }
}

#[pin_project(project = HybridFutureProj)]
pub enum HybridFuture<WebFuture, GrpcFuture> {
    Web(#[pin] WebFuture),
    Grpc(#[pin] GrpcFuture),
}

impl<WebFuture, GrpcFuture, WebBody, GrpcBody, WebError, GrpcError> Future
    for HybridFuture<WebFuture, GrpcFuture>
where
    WebFuture: Future<Output = Result<Response<WebBody>, WebError>>,
    GrpcFuture: Future<Output = Result<Response<GrpcBody>, GrpcError>>,
    WebError: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    GrpcError: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
{
    type Output = Result<
        Response<HybridBody<WebBody, GrpcBody>>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    >;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            HybridFutureProj::Web(web_future) => match web_future.poll(cx) {
                Poll::Ready(Ok(res)) => Poll::Ready(Ok(res.map(HybridBody::Web))),
                Poll::Ready(Err(e)) => Poll::Ready(Err(e.into())),
                Poll::Pending => Poll::Pending,
            },
            HybridFutureProj::Grpc(grpc_future) => match grpc_future.poll(cx) {
                Poll::Ready(Ok(res)) => Poll::Ready(Ok(res.map(HybridBody::Grpc))),
                Poll::Ready(Err(e)) => Poll::Ready(Err(e.into())),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[pin_project(project = HybridBodyProj)]
pub enum HybridBody<WebBody, GrpcBody> {
    Web(#[pin] WebBody),
    Grpc(#[pin] GrpcBody),
}

impl<WebBody, GrpcBody> HttpBody for HybridBody<WebBody, GrpcBody>
where
    WebBody: HttpBody + Send + Unpin,
    GrpcBody: HttpBody<Data = WebBody::Data> + Send + Unpin,
    WebBody::Error: std::error::Error + Send + Sync + 'static,
    GrpcBody::Error: std::error::Error + Send + Sync + 'static,
{
    type Data = WebBody::Data;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

    fn is_end_stream(&self) -> bool {
        match self {
            HybridBody::Web(b) => b.is_end_stream(),
            HybridBody::Grpc(b) => b.is_end_stream(),
        }
    }

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        match self.project() {
            HybridBodyProj::Web(b) => b.poll_data(cx).map_err(Into::into),
            HybridBodyProj::Grpc(b) => b.poll_data(cx).map_err(Into::into),
        }
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context,
    ) -> Poll<Result<Option<HeaderMap>, Self::Error>> {
        match self.project() {
            HybridBodyProj::Web(b) => b.poll_trailers(cx).map_err(Into::into),
            HybridBodyProj::Grpc(b) => b.poll_trailers(cx).map_err(Into::into),
        }
    }
}
