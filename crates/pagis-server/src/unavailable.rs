//! The route table supplies each absent Native Part's answer.

use crate::{error::ApiError, routes::ROUTES};
use axum::{
    Router,
    routing::{MethodFilter, MethodRouter},
};

pub(crate) fn product_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    let mut router = Router::new();
    for route in ROUTES.iter().filter(|route| route.part.is_some()) {
        let part = route.part.expect("the route names a Native Part");
        let mut methods = MethodRouter::new();
        for method in route.methods {
            let filter = MethodFilter::try_from(
                method
                    .to_uppercase()
                    .parse::<axum::http::Method>()
                    .expect("the route table names an HTTP method"),
            )
            .expect("axum supports the route table's method");
            methods = methods.on(filter, move || async move { ApiError::not_available(part) });
        }
        router = router.route(route.path, methods);
    }
    router
}
