//! Workers entrypoint. Thin by design: this file is Cloudflare-runtime glue
//! (Request/Env/Context, R2 bindings, live NWS fetch) that only executes
//! inside a deployed/dev worker, so it's excluded from the coverage gate.
//! All business logic lives in `handler`, `auth`, `models`, `weather`, `r2`
//! and is unit-tested there without touching the network or the runtime.
mod auth;
mod handler;
pub mod models;
mod r2;
mod r2_adapter;
pub mod version;
mod weather;
mod weather_adapter;

use auth::{API_KEY_HEADER, is_authorized};
use chrono::DateTime;
use handler::{VersionedHike, build_hike_response, build_locations_response};
use r2_adapter::{R2HikeStore, load_r2_config};
use version::{API_VERSION_HEADER, ApiVersion, gone_body, parse_version, sunset};
use weather_adapter::NwsWeatherSource;
use worker::*;

/// Routing is settled before admission: the catch-all answers an unrouted path
/// itself, and each handler's method guard runs ahead of its `API_KEY` read, so
/// a 501 or a 405 never depends on the key that came with the request.
// @spec API-ROUTE-005
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let router = Router::new();
    router
        .on_async("/health", |req, _| async move {
            if req.method() != Method::Get {
                return method_not_allowed();
            }
            Response::ok("ok")
        })
        .on_async("/hike/:id", |req, ctx| async move {
            handle_hike(req, ctx).await
        })
        // Both spellings of the id-less collection path reach the same handler,
        // which answers 404 once admission has run.
        .on_async(
            "/hike",
            |req, ctx| async move { handle_hike(req, ctx).await },
        )
        .on_async(
            "/hike/",
            |req, ctx| async move { handle_hike(req, ctx).await },
        )
        .on_async("/hike-locations", |req, ctx| async move {
            handle_hike_locations(req, ctx).await
        })
        // The bare root is registered alongside the wildcard because matchit's
        // catch-all needs a segment to bind to and would leave "/" unmatched.
        .or_else_any_method_async("/", |_, _| async { not_implemented() })
        .or_else_any_method_async("/*path", |_, _| async { not_implemented() })
        .run(req, env)
        .await
}

/// A path the router does not know. 404 is reserved for a hike that does not
/// exist, so an unknown URL says so in its own status code.
// @spec API-ROUTE-002
fn not_implemented() -> Result<Response> {
    Response::error("not implemented", 501)
}

/// A routed path under a method it does not serve. GET is the only method any
/// route serves, so `Allow` is constant. The `worker` Router's own 405 carries
/// no `Allow` and cannot be customised, so every route is registered for all
/// methods and guarded by its handler instead.
// @spec API-ROUTE-003
fn method_not_allowed() -> Result<Response> {
    let mut resp = Response::error("method not allowed", 405)?;
    resp.headers_mut().set("Allow", "GET")?;
    Ok(resp)
}

/// Reads and validates the `x-api-version` header. `Ok(version)` on success;
/// `Err(response)` is a ready-to-return 400 for an unsupported version, or a
/// 410 for one past its sunset. The 410 carries no `Sunset` header: the sunset
/// is the whole message and it is spelled out in the body (API-VER-007).
// @spec API-VER-001, API-VER-002, API-VER-005
fn negotiate_version(req: &Request) -> std::result::Result<ApiVersion, Response> {
    let header = req.headers().get(API_VERSION_HEADER).ok().flatten();
    let version = parse_version(header.as_deref()).map_err(|()| {
        Response::error("unsupported api version", 400)
            .unwrap_or_else(|_| Response::empty().unwrap())
    })?;

    // An unreadable clock falls back to the epoch, under which nothing is
    // sunset: a broken clock degrades to serving a version, never to 410-ing
    // every one of them.
    let now = DateTime::from_timestamp_millis(Date::now().as_millis() as i64).unwrap_or_default();
    match gone_body(version, now) {
        Some(body) => {
            Err(Response::error(body, 410).unwrap_or_else(|_| Response::empty().unwrap()))
        }
        None => Ok(version),
    }
}

/// Stamps RFC 8594 deprecation headers when the served version is deprecated.
/// A version with a registry sunset date carries these; the current one does not.
// @spec API-VER-006, API-VER-007
fn with_deprecation(mut resp: Response, version: ApiVersion) -> Result<Response> {
    if let Some(sunset_date) = sunset(version) {
        let headers = resp.headers_mut();
        headers.set("Deprecation", "true")?;
        headers.set("Sunset", sunset_date)?;
        headers.set(
            "Link",
            "<https://hike-club-api.scondon87.workers.dev/openapi.yaml>; rel=\"deprecation\"",
        )?;
    }
    Ok(resp)
}

// @spec API-ROUTE-003, API-AUTH-002, API-VER-003, API-LOC-002, API-LOC-004, API-ERR-001
async fn handle_hike_locations(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    if req.method() != Method::Get {
        return method_not_allowed();
    }

    let expected_key = match ctx.env.secret("API_KEY") {
        Ok(s) => s.to_string(),
        Err(_) => return Response::error("server misconfigured: API_KEY not set", 500),
    };
    let provided = req.headers().get(API_KEY_HEADER).ok().flatten();
    if !is_authorized(provided.as_deref(), &expected_key) {
        return Response::error("unauthorized", 401);
    }

    // Version-independent payload, but still negotiated so a bad version 400s.
    let version = match negotiate_version(&req) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };

    let config = match load_r2_config(&ctx.env) {
        Ok(c) => c,
        Err(e) => return Response::error(format!("server misconfigured: {e}"), 500),
    };
    let bucket = match ctx.env.bucket("HIKES") {
        Ok(b) => b,
        Err(e) => return Response::error(format!("server misconfigured: {e}"), 500),
    };
    let store = R2HikeStore {
        bucket,
        config: &config,
    };

    match build_locations_response(&store).await {
        (200, body) => {
            let mut resp = Response::ok(body)?;
            resp.headers_mut().set("content-type", "application/json")?;
            with_deprecation(resp, version)
        }
        (status, body) => Response::error(body, status),
    }
}

// @spec API-ROUTE-001, API-ROUTE-003, API-ROUTE-004, API-AUTH-001, API-AUTH-002, API-AUTH-003, API-AUTH-004, API-ERR-001, API-RESP-004, API-RESP-007, API-WIRE-006
async fn handle_hike(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    if req.method() != Method::Get {
        return method_not_allowed();
    }

    let expected_key = match ctx.env.secret("API_KEY") {
        Ok(s) => s.to_string(),
        Err(_) => return Response::error("server misconfigured: API_KEY not set", 500),
    };
    let provided = req.headers().get(API_KEY_HEADER).ok().flatten();
    if !is_authorized(provided.as_deref(), &expected_key) {
        return Response::error("unauthorized", 401);
    }

    let version = match negotiate_version(&req) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };

    let Some(id) = ctx.param("id") else {
        return Response::error("hike not found", 404);
    };

    let config = match load_r2_config(&ctx.env) {
        Ok(c) => c,
        Err(e) => return Response::error(format!("server misconfigured: {e}"), 500),
    };
    let bucket = match ctx.env.bucket("HIKES") {
        Ok(b) => b,
        Err(e) => return Response::error(format!("server misconfigured: {e}"), 500),
    };
    let store = R2HikeStore {
        bucket,
        config: &config,
    };
    let weather_source = NwsWeatherSource;

    match build_hike_response(&store, &weather_source, id, version).await {
        Ok(Some(VersionedHike::V2(r))) => with_deprecation(Response::from_json(&r)?, version),
        Ok(None) => Response::error("hike not found", 404),
        Err(e) => Response::error(format!("upstream error: {e}"), 502),
    }
}
