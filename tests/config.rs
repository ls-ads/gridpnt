//! Connection settings and how Databricks credentials are found.

use gridpnt::backend::databricks::auth::{Credentials, TokenSource, normalise_host, resolve};
use gridpnt::config::{self, Kind, parse_ini};

const HOST: &str = "https://adb-1.azuredatabricks.net";

#[test]
fn connections_are_read_from_ini_text() {
    let sections = parse_ini(
        "# my connections
         [work]
         type = databricks
         profile = DEFAULT      ; from ~/.databrickscfg
         warehouse = abc123
         env = prod
         pin = main.sales, main.finance ,analytics

         [scratch]
         type = sqlite
         path = /tmp/scratch.db",
    );
    let connections = config::connections(&sections).unwrap();

    assert_eq!(connections[0].name, "work");
    assert_eq!(connections[0].env.as_deref(), Some("prod"));
    let Kind::Databricks(work) = &connections[0].kind else {
        panic!("not databricks")
    };
    assert_eq!(work.profile.as_deref(), Some("DEFAULT"));
    assert_eq!(work.warehouse, "abc123");
    assert_eq!(work.pins, ["main.sales", "main.finance", "analytics"]);

    assert_eq!(connections[1].env, None);
    assert!(matches!(&connections[1].kind, Kind::Sqlite { path } if path.to_str() == Some("/tmp/scratch.db")));
}

#[test]
fn mistakes_in_a_connection_are_explained() {
    let missing = config::connections(&parse_ini("[work]\ntype = databricks")).unwrap_err();
    assert_eq!(missing, "connection [work] needs `warehouse = ...`");

    let unknown = config::connections(&parse_ini("[x]\ntype = oracle")).unwrap_err();
    assert_eq!(unknown, "connection [x] has unknown type `oracle`");
}

fn settings(host: Option<&str>, profile: Option<&str>) -> config::Databricks {
    config::Databricks {
        host: host.map(str::to_string),
        profile: profile.map(str::to_string),
        warehouse: "wh".into(),
        pins: Vec::new(),
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn a_profile_supplies_host_and_token() {
    let profiles = parse_ini(&format!("[DEFAULT]\nhost = {HOST}/\ntoken = dapi123"));
    let found = resolve(&settings(None, None), &no_env, &profiles).unwrap();
    assert_eq!(
        found,
        Credentials {
            host: HOST.into(),
            source: TokenSource::Fixed("dapi123".into())
        }
    );
}

#[test]
fn a_named_profile_must_exist() {
    let error = resolve(&settings(None, Some("WORK")), &no_env, &[]).unwrap_err();
    assert_eq!(error, "profile [WORK] was not found in .databrickscfg");
}

#[test]
fn a_token_is_never_used_with_a_different_host() {
    // The profile's token belongs to HOST; the connection points elsewhere.
    let profiles = parse_ini(&format!("[DEFAULT]\nhost = {HOST}\ntoken = dapi123"));
    let other = "https://other.cloud.databricks.com";
    let found = resolve(&settings(Some(other), None), &no_env, &profiles).unwrap();
    assert_eq!(
        found.source,
        TokenSource::Cli {
            host: other.into(),
            profile: None
        }
    );

    // The same holds for a token in the environment that names its host.
    let env = |name: &str| match name {
        "DATABRICKS_HOST" => Some(HOST.to_string()),
        "DATABRICKS_TOKEN" => Some("dapi-env".to_string()),
        _ => None,
    };
    let found = resolve(&settings(Some(other), None), &env, &[]).unwrap();
    assert_eq!(
        found.source,
        TokenSource::Cli {
            host: other.into(),
            profile: None
        }
    );
}

#[test]
fn the_environment_is_used_when_there_is_no_profile() {
    let env = |name: &str| match name {
        "DATABRICKS_HOST" => Some("adb-1.azuredatabricks.net".to_string()),
        "DATABRICKS_TOKEN" => Some("dapi-env".to_string()),
        _ => None,
    };
    let found = resolve(&settings(None, None), &env, &[]).unwrap();
    assert_eq!(found.host, HOST);
    assert_eq!(found.source, TokenSource::Fixed("dapi-env".into()));
}

#[test]
fn a_browser_login_profile_falls_back_to_the_cli() {
    // After `databricks auth login` the profile holds a host but no token.
    let profiles = parse_ini(&format!("[DEFAULT]\nhost = {HOST}\nauth_type = databricks-cli"));
    let found = resolve(&settings(None, None), &no_env, &profiles).unwrap();
    assert_eq!(
        found.source,
        TokenSource::Cli {
            host: HOST.into(),
            profile: Some("DEFAULT".into())
        }
    );
}

#[test]
fn no_host_anywhere_is_an_error() {
    assert!(
        resolve(&settings(None, None), &no_env, &[])
            .unwrap_err()
            .starts_with("no workspace address")
    );
}

#[test]
fn plain_http_is_refused_except_for_this_machine() {
    assert_eq!(normalise_host("adb-1.azuredatabricks.net/").unwrap(), HOST);
    assert_eq!(
        normalise_host("http://127.0.0.1:8080").unwrap(),
        "http://127.0.0.1:8080"
    );
    assert!(normalise_host("http://adb-1.azuredatabricks.net").is_err());
}
