//! Helpers for outbound control-plane HTTP requests with org context.

use reqwest::RequestBuilder;

use crate::store::users::LocalUser;

pub const ORG_HEADER: &str = "x-airnote-org-id";

pub fn with_org_context(builder: RequestBuilder, user: Option<&LocalUser>) -> RequestBuilder {
    with_org_id(builder, user.and_then(|u| u.active_org_id.as_deref()))
}

pub fn with_org_id(builder: RequestBuilder, active_org_id: Option<&str>) -> RequestBuilder {
    if let Some(org_id) = active_org_id.map(str::trim).filter(|s| !s.is_empty()) {
        builder.header(ORG_HEADER, org_id)
    } else {
        builder
    }
}
