//! Resolve versions using the workspace's own registry client and audit concrete public releases.
use super::{Change, Channel, Dependency, Error, Lens, Manifest, Upgrade, Versions, Workspace};
use crate::command::Cancellation;
use node_semver::{Range, Version};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const CACHE_AGE: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
struct Metadata {
    versions: Vec<String>,
    #[serde(rename = "dist-tags")]
    tags: HashMap<String, String>,
    #[serde(default)]
    dist: Value,
}

type CachedMetadata = HashMap<(Workspace, String), (Instant, Arc<Metadata>)>;

/// Short-lived registry data scoped to this document and its exact workspace.
#[derive(Default)]
pub struct Cache(Mutex<CachedMetadata>);

impl Cache {
    fn load(
        &self,
        ws: &Workspace,
        name: &str,
        cancel: &Cancellation,
    ) -> Result<Arc<Metadata>, Error> {
        let key = (ws.clone(), name.to_owned());
        if let Ok(data) = self.0.lock()
            && let Some((time, metadata)) = data.get(&key)
            && time.elapsed() < CACHE_AGE
        {
            return Ok(metadata.clone());
        }
        let folder = ws.folder();
        let output = match ws.manager.as_str() {
            "bun" => ws.run(&["info", name, "--json"], &folder, cancel, 25)?,
            "yarn" => {
                let version = ws.run(&["--version"], &folder, cancel, 10)?;
                if version.starts_with(b"1.") {
                    ws.run(&["info", name, "--json"], &folder, cancel, 25)?
                } else {
                    ws.run(
                        &[
                            "npm",
                            "info",
                            name,
                            "--fields",
                            "versions,dist-tags,dist",
                            "--json",
                        ],
                        &folder,
                        cancel,
                        25,
                    )?
                }
            }
            _ => ws.run(
                &[
                    "view",
                    name,
                    "versions",
                    "dist-tags",
                    "dist.tarball",
                    "--json",
                ],
                &folder,
                cancel,
                25,
            )?,
        };
        let mut value: Value = serde_json::Deserializer::from_slice(&output)
            .into_iter::<Value>()
            .filter_map(Result::ok)
            .find_map(|mut v| {
                if v.get("type").and_then(Value::as_str) == Some("inspect") {
                    v = v.get_mut("data")?.take()
                }
                v.get("versions")?;
                Some(v)
            })
            .ok_or_else(|| Error::Tool("Registry returned no versions".into()))?;
        if ws.manager == "bun" && value["dist-tags"].is_null() {
            value["dist-tags"] = json!({"latest":value["version"]})
        }
        if value["dist"].is_null() {
            value["dist"] = json!({"tarball":value["dist.tarball"]})
        }
        let metadata = Arc::new(
            serde_json::from_value(value)
                .map_err(|_| Error::Tool("Unreadable registry metadata".into()))?,
        );
        if let Ok(mut data) = self.0.lock() {
            data.retain(|_, (time, _)| time.elapsed() < CACHE_AGE);
            if data.len() < 2048 {
                data.insert(key, (Instant::now(), Arc::clone(&metadata)));
            }
        }
        Ok(metadata)
    }
}

/// Inspect an editor snapshot. An unavailable registry or lockfile leaves other packages usable.
pub fn inspect(
    ws: &Workspace,
    manifest: Manifest,
    cache: &Cache,
    mut progress: impl FnMut(&Lens),
    cancel: &Cancellation,
) -> Result<Vec<Lens>, Error> {
    let lock = ws.read_lock(cancel);
    let saved = ws.saved_dependencies(cancel);
    if let Err(e) = &lock {
        tracing::warn!(error=%e,"package lock could not be read")
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .into();
    let mut lenses = Vec::new();
    let mut checks = Vec::new();
    for d in manifest.entries {
        if cancel.is_cancelled() {
            return Err(Error::Tool("Cancelled".into()));
        }
        if !d.is_searchable() {
            continue;
        }
        let resolved = match &lock {
            Ok(Some(lock))
                if saved
                    .as_ref()
                    .and_then(|v| v.get(&d.group)?.get(&d.name)?.as_str())
                    == Some(&d.spec) =>
            {
                lock.resolved(&ws.importer, &d)
            }
            Ok(Some(_)) => None,
            Ok(None) => Version::parse(&d.spec).ok(),
            Err(_) => None,
        };
        let metadata = cache.load(ws, &d.name, cancel);
        let versions = match &metadata {
            Ok(data) => Versions::Found(upgrades(&d, data, resolved.as_ref())),
            Err(e) => Versions::Unavailable(e.to_string()),
        };
        if let (Ok(data), Some(v)) = (&metadata, &resolved)
            && public_tarball(data, &d.name)
            && (!d.name.starts_with('@') || anonymous_public(&agent, &d.name, v))
        {
            checks.push((lenses.len(), d.name.clone(), v.to_string()));
        }
        let lens = Lens {
            dependency: d,
            resolved,
            versions,
            advisories: Vec::new(),
            more_advisories: false,
        };
        progress(&lens);
        lenses.push(lens);
    }
    for chunk in checks.chunks(250) {
        if cancel.is_cancelled() {
            break;
        }
        let queries=chunk.iter().map(|(_,name,version)| {
            json!({"package":{"ecosystem":"npm","name":name},"version":version})
        }).collect::<Vec<_>>();
        match query_osv(&agent, queries).and_then(|batch| apply_audit(&mut lenses, chunk, batch)) {
            Ok(()) => {}
            Err(e) => tracing::warn!(error=%e,"package vulnerability lookup unavailable"),
        }
    }
    Ok(lenses)
}

fn query_osv(agent: &ureq::Agent, queries: Vec<Value>) -> Result<Batch, Error> {
    let mut response = agent
        .post("https://api.osv.dev/v1/querybatch")
        .header("Content-Type", "application/json")
        .send(json!({"queries":queries}).to_string().as_bytes())
        .map_err(|e| Error::Tool(format!("OSV request failed: {e}")))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| Error::Tool(format!("Cannot read OSV response: {e}")))?;
    serde_json::from_slice(&body).map_err(|_| Error::Tool("Invalid OSV response".into()))
}

fn apply_audit(
    lenses: &mut [Lens],
    checks: &[(usize, String, String)],
    batch: Batch,
) -> Result<(), Error> {
    if batch.results.len() != checks.len() {
        return Err(Error::Tool("Incomplete OSV response".into()));
    }
    for ((index, _, _), result) in checks.iter().zip(batch.results) {
        let Some(lens) = lenses.get_mut(*index) else {
            continue;
        };
        lens.advisories = result.vulns.into_iter().map(|v| v.id).collect();
        lens.more_advisories = result.next_page_token.is_some();
    }
    Ok(())
}

fn public_tarball(metadata: &Metadata, name: &str) -> bool {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    let Some(url) = metadata.dist["tarball"].as_str() else {
        return false;
    };
    [
        "https://registry.npmjs.org/",
        "https://registry.yarnpkg.com/",
    ]
    .iter()
    .any(|origin| {
        url.strip_prefix(origin)
            .is_some_and(|path| path.starts_with(&format!("{name}/-/{leaf}-")))
    })
}

fn anonymous_public(agent: &ureq::Agent, name: &str, version: &Version) -> bool {
    let path = name.replace('/', "%2F");
    agent
        .get(format!("https://registry.npmjs.org/{path}/{version}"))
        .call()
        .is_ok_and(|r| r.status().is_success())
}

#[derive(Deserialize)]
struct Batch {
    results: Vec<Audit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Audit {
    #[serde(default)]
    vulns: Vec<Advisory>,
    next_page_token: Option<String>,
}
#[derive(Deserialize)]
struct Advisory {
    id: String,
}

/// The newest release the requirement accepts, then the `latest` tag, when each moves it forward.
fn upgrades(d: &Dependency, data: &Metadata, resolved: Option<&Version>) -> Vec<Upgrade> {
    let range = Range::parse(&d.spec).ok();
    let base = Version::parse(d.spec.trim_start_matches(['^', '~'])).ok();
    let wanted = if let Some(range) = range {
        data.versions
            .iter()
            .filter_map(|s| Version::parse(s).ok())
            .filter(|v| range.satisfies(v))
            .max()
    } else {
        data.tags.get(&d.spec).and_then(|s| Version::parse(s).ok())
    };
    let latest = data.tags.get("latest").and_then(|s| Version::parse(s).ok());
    let mut upgrades: Vec<Upgrade> = Vec::new();
    for (channel, candidate) in [(Channel::Wanted, wanted), (Channel::Latest, latest)] {
        let Some(version) = candidate else { continue };
        let spec = d.replacement(&version);
        if spec == d.spec
            || base.as_ref().is_some_and(|b| version < *b)
            || upgrades.iter().any(|u| u.spec == spec)
        {
            continue;
        }
        let change = base
            .as_ref()
            .or(resolved)
            .and_then(|from| Change::between(from, &version));
        upgrades.push(Upgrade {
            channel,
            version,
            change,
            spec,
        });
    }
    upgrades
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advisory_ids_and_unknown_results_stay_with_the_queried_version() {
        let checks = [(0, "@scope/pkg".into(), "1.2.0".into())];
        let mut rows = [lens("^1.0.0")];
        let incomplete = serde_json::from_str(r#"{"results":[]}"#).unwrap();
        assert!(apply_audit(&mut rows, &checks, incomplete).is_err());
        assert!(rows[0].advisories.is_empty());
        let hit = serde_json::from_str(
            r#"{"results":[{"vulns":[{"id":"GHSA-test","modified":"today"}]}]}"#,
        )
        .unwrap();
        apply_audit(&mut rows, &checks, hit).unwrap();
        assert_eq!(rows[0].advisories, ["GHSA-test"]);
        assert!(!rows[0].more_advisories);
        let paged = serde_json::from_str(
            r#"{"results":[{"vulns":[{"id":"GHSA-a"}],"next_page_token":"next"}]}"#,
        )
        .unwrap();
        apply_audit(&mut rows, &checks, paged).unwrap();
        assert_eq!(
            (rows[0].advisories.as_slice(), rows[0].more_advisories),
            (["GHSA-a".to_owned()].as_slice(), true)
        );
    }
    fn dependency(spec: &str) -> Dependency {
        let text = json!({"dependencies":{"@scope/pkg":spec}}).to_string();
        Manifest::parse(&text).unwrap().entries.remove(0)
    }
    fn lens(spec: &str) -> Lens {
        Lens {
            dependency: dependency(spec),
            resolved: None,
            versions: Versions::Found(Vec::new()),
            advisories: Vec::new(),
            more_advisories: false,
        }
    }
    #[test]
    fn proposes_the_maximum_compatible_and_dist_tagged_release_without_prerelease_drift() {
        let data = Metadata {
            versions: ["1.2.0", "1.4.2", "1.8.0-beta.1", "2.0.0", "3.1.0"]
                .map(String::from)
                .to_vec(),
            tags: HashMap::from([("latest".into(), "2.0.0".into())]),
            dist: Value::Null,
        };
        use Change::{Major, Minor};
        use Channel::{Latest, Wanted};
        for (spec, expected) in [
            (
                "^1.2.0",
                vec![
                    (Wanted, "^1.4.2", Some(Minor)),
                    (Latest, "^2.0.0", Some(Major)),
                ],
            ),
            ("~1.2.0", vec![(Latest, "~2.0.0", Some(Major))]),
            ("1.2.0", vec![(Latest, "2.0.0", Some(Major))]),
            // No exact version in the range: the step is unknown.
            (
                ">=1 <2",
                vec![(Wanted, "1.4.2", None), (Latest, "2.0.0", None)],
            ),
            ("^2.0.0", vec![]),
            // A requirement ahead of `latest` is never offered a downgrade.
            ("^3.0.0", vec![(Wanted, "^3.1.0", Some(Minor))]),
        ] {
            let offered = upgrades(&dependency(spec), &data, None)
                .into_iter()
                .map(|u| (u.channel, u.spec, u.change))
                .collect::<Vec<_>>();
            let expected = expected
                .into_iter()
                .map(|(channel, spec, change)| (channel, spec.to_owned(), change))
                .collect::<Vec<_>>();
            assert_eq!(offered, expected, "{spec}");
        }
        // A range without an exact version measures the step from the locked release.
        let locked = Version::parse("1.4.0").unwrap();
        let steps = upgrades(&dependency(">=1 <2"), &data, Some(&locked))
            .into_iter()
            .map(|u| u.change)
            .collect::<Vec<_>>();
        assert_eq!(steps, [Some(Change::Patch), Some(Change::Major)]);
    }
    #[test]
    fn custom_registry_tarballs_do_not_enter_the_public_audit() {
        let mut data = Metadata {
            versions: Vec::new(),
            tags: HashMap::new(),
            dist: Value::Null,
        };
        for (url, public) in [
            (
                "https://registry.npmjs.org/@scope/pkg/-/pkg-1.0.0.tgz",
                true,
            ),
            (
                "https://registry.npmjs.org.example.com/@scope/pkg/-/pkg-1.0.0.tgz",
                false,
            ),
            ("https://private.example/@scope/pkg/-/pkg-1.0.0.tgz", false),
        ] {
            data.dist = json!({"tarball":url});
            assert_eq!(public_tarball(&data, "@scope/pkg"), public);
        }
    }
}
