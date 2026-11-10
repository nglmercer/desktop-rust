//! GitHub REST/GraphQL client — port of `app/src/lib/api.ts` + `http.ts`.

pub mod client;
pub mod endpoints;

pub use client::ApiClient;
pub use endpoints::{
    DOTCOM_API_ENDPOINT, DOTCOM_HTML_URL, get_absolute_url, get_dot_com_api_endpoint,
    get_endpoint_for_repository, get_enterprise_api_url, get_html_url,
    get_next_page_path_from_link, get_oauth_authorization_url,
};
