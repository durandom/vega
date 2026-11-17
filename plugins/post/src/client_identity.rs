//! Fresh-host native v2 enrollment, separate from the legacy Unix cutover.
use crate::client_key::{check_path, create_private, read_private};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use vega_herdr::{platform, project, Failure};

#[derive(Subcommand)]
pub enum Action {
    /// Enroll a fresh host after operator nkey/inbox provisioning; safely repeat after interruption
    Enroll {
        /// Owner-local checkout to resolve to v2 identity (repeat for each repository)
        #[arg(long, required = true)]
        checkout: Vec<PathBuf>,
    },
    /// Verify the durable local enrollment and broker projection
    Verify,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Enrollment {
    schema: u32,
    host: String,
    public_key: String,
    projects: Vec<project::Project>,
}
fn fail(e: impl std::fmt::Display) -> Failure {
    Failure::new("identity_enrollment",e.to_string(),"use a fresh native host with its own nkey; ask the operator to reconcile a conflicting identity; see docs/design/windows.md")
}
fn root() -> Result<PathBuf, Failure> {
    platform::state_home()
        .map(|p| p.join("vega"))
        .ok_or_else(|| fail("no state home"))
}
fn record() -> Result<PathBuf, Failure> {
    Ok(root()?.join("identity-native/enrollment.json"))
}
fn load() -> Result<Enrollment, Failure> {
    serde_json::from_slice(&read_private(&record()?)?).map_err(fail)
}
fn current_key() -> Result<String, Failure> {
    let path = crate::seed_path(crate::Seed::Host)?;
    let seed = crate::client_key::read_seed(std::path::Path::new(&path))?;
    Ok(nkeys::KeyPair::from_seed(&seed).map_err(fail)?.public_key())
}
fn validate(enrollment: &Enrollment) -> Result<(), Failure> {
    if enrollment.schema != 2
        || enrollment.host != crate::host()
        || enrollment.public_key != current_key()?
        || enrollment.projects.is_empty()
    {
        return Err(fail("enrollment host/schema/key does not match this owner"));
    }
    for p in &enrollment.projects {
        if project::resolve(&p.location.checkout, &enrollment.host)? != *p {
            return Err(fail(
                "checkout changed since enrollment; explicit owner rekey required",
            ));
        }
    }
    Ok(())
}
/// Resident workers may publish v2 presence only for this enrolled owner.
pub fn verify_local() -> Result<(), Failure> {
    validate(&load()?)
}
pub fn run(action: Action) -> Result<(), Failure> {
    let host = crate::host();
    if vega_herdr::state::is_inbox_only(&host) {
        return Err(fail("native enrollment requires normal host permissions"));
    }
    let file = record()?;
    check_path(&file)?;
    let enroll = matches!(&action, Action::Enroll { .. });
    let enrollment = match action {
        Action::Enroll { checkout } => {
            // Never turn imported Unix state into a fresh-host completion marker.
            for name in ["identity-cutover", "uac", "lanes"] {
                if root()?.join(name).exists() {
                    return Err(fail(format!(
                        "existing {name} state needs operator migration"
                    )));
                }
            }
            let mut projects = checkout
                .iter()
                .map(|p| project::resolve(p, &host))
                .collect::<Result<Vec<_>, _>>()?;
            projects.sort_by(|a, b| a.location.checkout.cmp(&b.location.checkout));
            projects.dedup();
            let proposed = Enrollment {
                schema: 2,
                host: host.clone(),
                public_key: current_key()?,
                projects,
            };
            if file.exists() {
                if load()? != proposed {
                    return Err(fail("enrollment already exists with a different identity"));
                }
            } else {
                create_private(&file, &serde_json::to_vec_pretty(&proposed).map_err(fail)?)?;
            }
            proposed
        }
        Action::Verify => load()?,
    };
    validate(&enrollment)?;
    crate::runtime()?.block_on(async {
        let client = crate::connect("identity enrollment", None).await?;
        let js = vega_registry::broker_context(client.clone());
        // Authenticated hosts cannot create inbox streams; provisioning stays operator-only.
        crate::inbox_stream(&js, &host).await?;
        let kv = vega_registry::bucket(&client).await.map_err(fail)?;
        let key = vega_registry::identity_key(&host);
        let expected = serde_json::to_value(&enrollment).map_err(fail)?;
        let view = vega_registry::read_kinds(&client, &kv, Some(&[]))
            .await
            .map_err(fail)?;
        for candidate in view.keys {
            if candidate.starts_with(&format!("{host}."))
                && vega_registry::is_legacy_project_key(&candidate)
                && vega_registry::get_one(&client, &candidate)
                    .await
                    .map_err(fail)?
                    .is_some()
            {
                return Err(fail(format!(
                    "legacy owned registry key {candidate}; refusing fresh enrollment"
                )));
            }
        }
        let existing = vega_registry::get_one(&client, &key).await.map_err(fail)?;
        match existing {
            Some(value) if value != expected => {
                return Err(fail("broker identity differs; refusing overwrite"))
            }
            None if enroll => {
                kv.create(key.clone(), expected.to_string().into())
                    .await
                    .map_err(fail)?;
            }
            None => return Err(fail("broker enrollment is missing; repeat identity enroll")),
            _ => {}
        }
        if vega_registry::get_one(&client, &key)
            .await
            .map_err(fail)?
            .as_ref()
            != Some(&expected)
        {
            return Err(fail("broker projection verification failed"));
        }
        println!("identity: v2 enrolled and verified for {host}");
        println!("record: {}", file.display());
        println!("registry: {key}");
        println!(
            "inbox: {} (reply prefix {})",
            crate::stream_name(&host),
            crate::inbox_prefix(&host)
        );
        Ok(())
    })
}
