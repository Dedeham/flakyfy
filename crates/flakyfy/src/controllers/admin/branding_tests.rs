//! Changing the logo: who may, what is taken, and that the usual address then shows it.

use std::io::Cursor;
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use flakyfy_core::settings::{AppSettings, SubsonicSettings};
use http_body_util::BodyExt;
use image::{DynamicImage, ImageFormat, RgbaImage};
use tower::ServiceExt;

use super::super::test_app::{Reply, admin_write, app, get};
use crate::app::{AppState, TestServices};
use crate::http::pipeline::{App, app_routes, build_with};
use crate::http::static_files::{StaticAsset, StaticAssets};

const LOGO: &str = "/api/admin/branding/logo";
const PICTURE_URL: &str = "/Assets/flakyfy_logo.png";

fn settings() -> AppSettings {
    AppSettings {
        subsonic: SubsonicSettings {
            url: Some("http://127.0.0.1:1".into()),
            admin_username: Some("admin".into()),
            admin_password: Some("synthetic-admin-password".into()),
            auto_detect_download_path: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::new(16, 16))
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode");
    bytes.into_inner()
}

async fn post_file(app: &App, file: &[u8], with_header: bool) -> Reply {
    let boundary = "flakyfy-test-boundary";
    let mut body =
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"logo\"\r\n\r\n")
            .into_bytes();
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut request = Request::builder().method(Method::POST).uri(LOGO).header(
        "content-type",
        format!("multipart/form-data; boundary={boundary}"),
    );
    if with_header {
        request = request.header("X-Octo-Admin", "1");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).expect("request"))
        .await
        .expect("infallible");
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.expect("body").to_bytes().to_vec(),
    }
}

#[tokio::test]
async fn changing_the_logo_needs_the_admin_header() {
    let app = app(AppState::for_tests(settings()));
    assert_eq!(post_file(&app, &png(), false).await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn changing_the_logo_needs_the_sign_in() {
    let state = AppState::for_tests_with(
        settings(),
        TestServices {
            sign_in: Some("required".into()),
            ..Default::default()
        },
    );
    let app = app(state);
    assert_eq!(
        post_file(&app, &png(), true).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(get(&app, LOGO).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn what_is_not_a_picture_is_refused_and_nothing_is_kept() {
    let state = AppState::for_tests(settings());
    let app = app(state.clone());
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>";
    assert_eq!(post_file(&app, svg, true).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        post_file(&app, b"nope", true).await.status,
        StatusCode::BAD_REQUEST
    );
    assert!(!state.cover_art_service.has_custom_logo());
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused() {
    let state = AppState::for_tests(settings());
    let app = app(state.clone());
    let large = vec![0u8; flakyfy_media::cover::custom_logo::MAX_UPLOAD_BYTES + 1];
    assert_eq!(
        post_file(&app, &large, true).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(!state.cover_art_service.has_custom_logo());
}

#[tokio::test]
async fn the_usual_address_shows_the_upload_until_it_is_reset() {
    let state = AppState::for_tests(settings());
    let mut assets = StaticAssets::default();
    assets.push(StaticAsset::new(
        PICTURE_URL.to_string(),
        "image/png",
        b"bundled".to_vec(),
        UNIX_EPOCH,
    ));
    let app = build_with(state.clone(), app_routes(&assets));
    assert_eq!(get(&app, PICTURE_URL).await.body, b"bundled");
    assert_eq!(get(&app, LOGO).await.json()["custom"], false);

    assert_eq!(post_file(&app, &png(), true).await.status, StatusCode::OK);
    let shown = get(&app, PICTURE_URL).await;
    assert_eq!(shown.header("content-type"), Some("image/png"));
    assert_eq!(image::guess_format(&shown.body).ok(), Some(ImageFormat::Png));
    assert_eq!(get(&app, LOGO).await.json()["custom"], true);

    let reset = admin_write(&app, Method::DELETE, LOGO, None).await;
    assert_eq!(reset.status, StatusCode::OK);
    assert_eq!(get(&app, PICTURE_URL).await.body, b"bundled");
    assert!(!state.cover_art_service.has_custom_logo());
}
