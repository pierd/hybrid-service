mod simple;
#[cfg(feature = "axum")]
mod with_conn_info;

pub use simple::HybridMakeService;
#[cfg(feature = "axum")]
pub use with_conn_info::{ConnectInfo, HybridMakeServiceWithConnInfo};

pub fn hybrid<MakeWeb, Grpc>(make_web: MakeWeb, grpc: Grpc) -> HybridMakeService<MakeWeb, Grpc> {
    HybridMakeService::new(make_web, grpc)
}

#[cfg(feature = "axum")]
pub fn hybrid_with_conn_info<MakeWeb, Grpc, ConnInfo>(
    make_web: MakeWeb,
    grpc: Grpc,
) -> with_conn_info::HybridMakeServiceWithConnInfo<MakeWeb, Grpc, ConnInfo> {
    with_conn_info::HybridMakeServiceWithConnInfo::new(make_web, grpc)
}
