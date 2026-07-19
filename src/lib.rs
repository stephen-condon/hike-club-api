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
mod weather;
mod weather_adapter;

use auth::{API_KEY_HEADER, is_authorized};
use handler::build_hike_response;
use r2_adapter::{R2HikeStore, load_r2_config};
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

async fn handle_hike_locations(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let expected_key = match ctx.env.secret("API_KEY") {
        Ok(s) => s.to_string(),
        Err(_) => return Response::error("server misconfigured: API_KEY not set", 500),
    };
    let provided = req.headers().get(API_KEY_HEADER).ok().flatten();
    if !is_authorized(provided.as_deref(), &expected_key) {
        return Response::error("unauthorized", 401);
    }

    let mut resp = Response::ok(HIKE_LOCATIONS_JSON)?;
    resp.headers_mut().set("content-type", "application/json")?;
    Ok(resp)
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

    match build_hike_response(&store, &weather_source, id).await {
        Ok(Some(response)) => Response::from_json(&response),
        Ok(None) => Response::error("hike not found", 404),
        Err(e) => Response::error(format!("upstream error: {e}"), 502),
    }
}
