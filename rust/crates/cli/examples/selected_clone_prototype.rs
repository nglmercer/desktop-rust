//! Test-only entry point for the unexposed selected-account clone prototype.
use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "__credential-helper")
    {
        return desktop_core::git_credential_helper(
            arguments.get(2).map(String::as_str).unwrap_or(""),
        )
        .await;
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "recover")
    {
        anyhow::ensure!(arguments.len() == 4, "expected registry and recovery ID");
        let record = desktop_core::recover_selected_clone(
            desktop_core::Registry::new(arguments[2].clone().into()),
            &arguments[3],
        )
        .await?;
        println!("{}", record.path.display());
        return Ok(());
    }
    anyhow::ensure!(
        arguments.len() == 7,
        "expected registry, URL, destination, host, login and numeric account ID"
    );
    let cancellation = desktop_core::cancellation::Cancellation::default();
    let mut signal = desktop_core::cancellation::ShutdownSignal::new()?;
    let cancel_on_signal = cancellation.clone();
    let watcher = tokio::spawn(async move {
        let _ = signal.received().await;
        cancel_on_signal.cancel();
    });
    let result = cancellation
        .scope(desktop_core::clone_repository_selected(
            desktop_core::Registry::new(arguments[1].clone().into()),
            &arguments[2],
            Path::new(&arguments[3]),
            desktop_core::CloneOptions::default(),
            desktop_core::CloneAccount {
                host: arguments[4].clone(),
                login: arguments[5].clone(),
                id: arguments[6].parse()?,
            },
        ))
        .await;
    watcher.abort();
    let repository = result?;
    println!("{}", repository.path().display());
    Ok(())
}
