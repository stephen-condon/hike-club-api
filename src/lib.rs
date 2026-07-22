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
use handler::{VersionedHike, build_hike_response};
use r2_adapter::{R2HikeStore, load_r2_config};
use version::{API_VERSION_HEADER, ApiVersion, parse_version, sunset};
use weather_adapter::NwsWeatherSource;
use worker::*;

/// Short-name → full-name mapping for the club's forest preserves, embedded at
/// compile time and served verbatim by `GET /hike-locations`.
const HIKE_LOCATIONS_JSON: &str = include_str!("../resources/hike-location-mapping.json");

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let router = Router::new();
    router
        .get_async("/health", |_, _| async { Response::ok("ok") })
        .get_async("/hike/:id", |req, ctx| async move {
            handle_hike(req, ctx).await
        })
        .get_async("/hike-locations", |req, ctx| async move {
            handle_hike_locations(req, ctx).await
        })
        .run(req, env)
        .await
}

/// Reads and validates the `x-api-version` header. `Ok(version)` on success;
/// `Err(response)` is a ready-to-return 400 for an unsupported version.
fn negotiate_version(req: &Request) -> std::result::Result<ApiVersion, Response> {
    let header = req.headers().get(API_VERSION_HEADER).ok().flatten();
    parse_version(header.as_deref()).map_err(|()| {
        Response::error("unsupported api version", 400)
            .unwrap_or_else(|_| Response::empty().unwrap())
    })
}

/// Stamps RFC 8594 deprecation headers when the served version is deprecated.
/// v1 responses carry these; v2 responses don't. See `version::sunset`.
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

async fn handle_hike_locations(req: Request, ctx: RouteContext<()>) -> Result<Response> {
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

    let mut resp = Response::ok(HIKE_LOCATIONS_JSON)?;
    resp.headers_mut().set("content-type", "application/json")?;
    with_deprecation(resp, version)
}

async fn handle_hike(req: Request, ctx: RouteContext<()>) -> Result<Response> {
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
        return Response::error("missing hike id", 400);
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
        Ok(Some(VersionedHike::V1(r))) => with_deprecation(Response::from_json(&r)?, version),
        Ok(Some(VersionedHike::V2(r))) => with_deprecation(Response::from_json(&r)?, version),
        Ok(None) => Response::error("hike not found", 404),
        Err(e) => Response::error(format!("upstream error: {e}"), 502),
    }
}
