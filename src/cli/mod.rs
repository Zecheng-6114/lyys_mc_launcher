use crate::core::auth::{AccountSession, Auth, AuthError, SessionState};

const USAGE: &str = "usage:
  lyys_mc_launcher account login
  lyys_mc_launcher account offline <name>
  lyys_mc_launcher account list
  lyys_mc_launcher account use <uuid>
  lyys_mc_launcher account remove <uuid>
  lyys_mc_launcher account current";

const ACCOUNT_COMMAND: &str = "account";

pub async fn run() -> Result<(), AuthError> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or_else(|| AuthError::InvalidArgument(USAGE.to_owned()))?;
    if command != ACCOUNT_COMMAND {
        return Err(AuthError::InvalidArgument(format!(
            "unknown command {command}\n{USAGE}"
        )));
    }
    let action = args
        .next()
        .ok_or_else(|| AuthError::InvalidArgument(USAGE.to_owned()))?;
    let auth = Auth::new()?;
    match action.as_str() {
        "login" => {
            let session = auth
                .login(|prompt| {
                    println!(
                        "open {} in a browser and enter code {}",
                        prompt.verification_uri, prompt.user_code
                    );
                    println!("the code is valid for {} minutes", prompt.expires_in / 60);
                })
                .await?;
            println!(
                "logged in: {} ({})",
                session.profile.name, session.profile.id
            );
        }
        "list" => print_accounts(&auth)?,
        "offline" => {
            let name = take_name(&mut args)?;
            let session = auth.add_offline(&name)?;
            println!(
                "offline account added: {} ({})",
                session.profile.name, session.profile.id
            );
        }
        "use" => {
            let uuid = take_uuid(&mut args)?;
            auth.set_active(&uuid)?;
            println!("active account set to {uuid}");
        }
        "remove" => {
            let uuid = take_uuid(&mut args)?;
            auth.remove(&uuid)?;
            println!("account removed: {uuid}");
        }
        "current" => {
            let session = auth.refresh_current().await?;
            println!("{}", account_line("*", &session));
        }
        other => {
            return Err(AuthError::InvalidArgument(format!(
                "unknown account command {other}\n{USAGE}"
            )));
        }
    }
    Ok(())
}

fn take_uuid(args: &mut impl Iterator<Item = String>) -> Result<String, AuthError> {
    args.next()
        .ok_or_else(|| AuthError::InvalidArgument("missing account uuid".to_owned()))
}

fn take_name(args: &mut impl Iterator<Item = String>) -> Result<String, AuthError> {
    args.next()
        .ok_or_else(|| AuthError::InvalidArgument("missing offline username".to_owned()))
}

fn print_accounts(auth: &Auth) -> Result<(), AuthError> {
    let accounts = auth.list()?;
    if accounts.is_empty() {
        println!("no accounts yet, run account login to add one");
        return Ok(());
    }
    let active = auth.active_uuid()?;
    for account in accounts {
        let marker = if active.as_deref() == Some(account.profile.id.as_str()) {
            "*"
        } else {
            " "
        };
        println!("{}", account_line(marker, &account));
    }
    Ok(())
}

fn account_line(marker: &str, account: &AccountSession) -> String {
    let state = match account.state {
        SessionState::Valid => "valid",
        SessionState::NeedsRelogin => "needs relogin",
        SessionState::Offline => "offline",
    };
    format!(
        "{marker} {}  {}  {state}",
        account.profile.name, account.profile.id
    )
}
