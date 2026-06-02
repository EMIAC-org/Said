//! Sentry crash + error telemetry. Opt-out, on by default.
//!
//! Disabled when any of:
//!   - `SENTRY_DSN` env var is unset (no build-time DSN baked in)
//!   - `SAID_DISABLE_SENTRY=1` env var is set (env override, highest priority)
//!   - `desktop_prefs.json::sentry_disabled` is `true` (user toggle in Settings)
//!
//! When enabled, captures:
//!   - panic stacktraces
//!   - `tracing::error!` events (warn and below are ignored)
//!
//! Tags every event with: app version, os, arch, anonymous device id,
//! release channel (stable / beta).
//!
//! What we never send:
//!   - transcript text, polished text, audio, API keys
//!   - file paths inside the user home dir (scrubbed to `~/...` by the
//!     `before_send` hook below)

use sentry::{ClientInitGuard, ClientOptions};

use crate::{paths, scrub};

/// Initialise Sentry for a service. Returns `None` when telemetry is
/// disabled by env, in which case the caller's panic hook + tracing setup
/// behave exactly as they did before. The returned guard must be held for
/// the lifetime of the process — drop it and events stop flushing.
pub fn init(service: &str) -> Option<ClientInitGuard> {
    // Env-var kill switch wins (lets users disable from a shell without
    // editing the prefs file).
    let env_disabled = std::env::var("SAID_DISABLE_SENTRY")
        .ok()
        .filter(|v| !v.is_empty() && v != "0")
        .is_some();

    let user_prefs = crate::prefs::load();

    if env_disabled || user_prefs.sentry_disabled {
        return None;
    }

    let dsn = std::env::var("SENTRY_DSN").ok().filter(|s| !s.is_empty())?;

    let release = format!("said@{}", env!("CARGO_PKG_VERSION"));
    // Channel tag on the Sentry event. Env override > prefs file > stable.
    let channel =
        std::env::var("SAID_CHANNEL").unwrap_or_else(|_| user_prefs.update_channel.clone());

    let guard = sentry::init((
        dsn,
        ClientOptions {
            release: Some(release.into()),
            environment: Some(channel.into()),
            attach_stacktrace: true,
            send_default_pii: false,
            before_send: Some(std::sync::Arc::new(scrub_event)),
            ..Default::default()
        },
    ));

    sentry::configure_scope(|scope| {
        scope.set_tag("service", service);
        scope.set_tag("os", std::env::consts::OS);
        scope.set_tag("arch", std::env::consts::ARCH);
        scope.set_user(Some(sentry::User {
            id: Some(paths::device_id()),
            ..Default::default()
        }));
    });

    Some(guard)
}

/// Build a tracing layer that forwards `error!` events to Sentry.
/// Warn and info are ignored so we don't ship every startup health-check
/// retry to the diagnostics pipeline.
pub fn tracing_layer<S>() -> sentry_tracing::SentryLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    sentry_tracing::layer().event_filter(|md| match *md.level() {
        tracing::Level::ERROR => sentry_tracing::EventFilter::Event,
        _ => sentry_tracing::EventFilter::Ignore,
    })
}

/// PII / path scrubber. Runs on every event before it leaves the process.
///
/// Currently redacts:
///   - file paths inside `$HOME` rewritten to `~/...`
///   - the user's username inside arbitrary strings
///
/// Strings are inspected in: event message, exception values, breadcrumb
/// messages. We intentionally don't rummage through every JSON leaf —
/// keeps the redaction surface small and reviewable. Add more passes if a
/// PII leak shows up in a sample event.
fn scrub_event(
    mut event: sentry::protocol::Event<'static>,
) -> Option<sentry::protocol::Event<'static>> {
    if let Some(ref mut msg) = event.message {
        scrub::scrub_string(msg);
    }

    for exc in event.exception.values.iter_mut() {
        if let Some(ref mut val) = exc.value {
            scrub::scrub_string(val);
        }
    }

    for bc in event.breadcrumbs.values.iter_mut() {
        if let Some(ref mut m) = bc.message {
            scrub::scrub_string(m);
        }
    }

    Some(event)
}
