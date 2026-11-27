//! Fixed Git credential-helper execution for the renderer trampoline.
//!
//! The helper command and policy environment are host-owned; only the credential
//! protocol input and the two UI behavior flags cross the typed boundary.
use crate::{Repository, args};
use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::collections::BTreeMap;

impl Repository {
    pub(crate) async fn renderer_credential(
        &self,
        command: &str,
        input_base64: &str,
        interactive: bool,
        software_rendering: bool,
    ) -> Result<Value> {
        anyhow::ensure!(
            matches!(command, "fill" | "approve" | "reject"),
            "unsupported credential operation"
        );
        anyhow::ensure!(
            input_base64.len() <= 384 * 1024,
            "credential input is too large"
        );
        let input = STANDARD
            .decode(input_base64)
            .map_err(|_| anyhow::anyhow!("credential input is not valid base64"))?;
        anyhow::ensure!(
            input.len() <= 256 * 1024,
            "decoded credential input is too large"
        );
        let mut environment = BTreeMap::new();
        environment.insert("GIT_TERMINAL_PROMPT".to_string(), Some("0".to_string()));
        environment.insert("GIT_ASKPASS".to_string(), Some(String::new()));
        environment.insert("TERM".to_string(), Some("dumb".to_string()));
        environment.insert(
            "GCM_INTERACTIVE".to_string(),
            Some(if interactive { "1" } else { "0" }.to_string()),
        );
        if software_rendering {
            environment.insert(
                "GCM_GUI_SOFTWARE_RENDERING".to_string(),
                Some("1".to_string()),
            );
        }
        let output = crate::process::run_git_with_environment_limit(
            &self.path,
            &args(&[
                "-c",
                "credential.helper=",
                "-c",
                "credential.helper=manager",
                "credential",
                command,
            ]),
            Some(input),
            None,
            &environment,
            1024 * 1024,
        )
        .await?;
        if output.code != 0 {
            // Credential stderr can contain user-facing helper diagnostics but no
            // repository data is needed for the legacy error contract.
            anyhow::bail!(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(json!({"bytes_base64":STANDARD.encode(output.stdout)}))
    }
}
