use axum::{Json, extract::State};

use crate::{AppState, get_prefs_cached, stt::bias};

pub async fn get_bias(
    State(state): State<AppState>,
) -> Json<said_core::deepgram::BiasPackage> {
    let user_id = state.default_user_id.clone();
    let prefs = get_prefs_cached(&state.prefs_cache, &state.pool, &user_id).await;
    let package = match prefs {
        Some(prefs) => bias::build_bias_package(
            &state.pool,
            &user_id,
            &prefs.language,
            &prefs.output_language,
        ),
        None => said_core::deepgram::BiasPackage::default(),
    };
    Json(package)
}
