use super::*;
fn output(code: i32, stdout: &str, stderr: &str) -> command::Output {
    command::Output {
        code: Some(code),
        stdout: stdout.as_bytes().into(),
        stderr: stderr.as_bytes().into(),
    }
}
#[test]
fn server_rejects_urls_options_shell_input_and_invalid_ports() {
    for invalid in [
        "",
        "https://gitlab.com",
        "-host",
        "foo/bar",
        "user@host",
        "foo;bar",
        "foo bar",
        "a..b",
        "host:0",
        "host:65536",
        "host:abc",
        "host\n--flag",
        "host:",
    ] {
        assert!(Server::parse(invalid).is_err(), "{invalid}");
    }
    for valid in ["gitlab.com", "git.example.com:8443", "localhost"] {
        assert_eq!(Server::parse(valid).unwrap().as_str(), valid);
    }
    assert_eq!(
        Server::parse(" GitLab.COM ").unwrap().as_str(),
        "gitlab.com"
    );
}
#[test]
fn identity_comes_from_provider_and_unknown_fields_are_ignored() {
    assert_eq!(
        account_response(
            Provider::GitHub,
            output(0, r#"{"login":"octocat","future":1}"#, " ")
        )
        .unwrap(),
        Connection::Connected("octocat".into())
    );
    assert_eq!(
        account_response(
            Provider::GitLab,
            output(0, r#"{"username":"person","id":42}"#, " ")
        )
        .unwrap(),
        Connection::Connected("person".into())
    );
    for value in [
        "",
        "{",
        "null",
        "{}",
        r#"{"login":42}"#,
        r#"{"login":"bad\nname"}"#,
    ] {
        assert!(account_response(Provider::GitHub, output(0, value, "")).is_err());
    }
}
#[test]
fn sign_in_and_network_failures_remain_distinct_without_raw_output() {
    for message in [
        "HTTP 401",
        "To get started, run gh auth login",
        "You are not logged in",
    ] {
        assert_eq!(
            account_response(Provider::GitHub, output(1, "", message)).unwrap(),
            Connection::SignedOut
        );
    }
    assert!(matches!(
        account_response(Provider::GitLab, output(1, "", "403 Forbidden")),
        Err(Error::AccessDenied)
    ));
    assert!(matches!(
        account_response(Provider::GitLab, output(1, "", "503 secret-value")),
        Err(Error::Unavailable)
    ));
}
#[test]
fn device_code_is_delivered_only_with_a_complete_same_host_https_url() {
    let server = Server::parse("github.com").unwrap();
    let code = "! First copy your one-time code: ABCD-1234\n";
    assert!(device_prompt(code.as_bytes(), &server).is_none());
    for url in [
        "http://github.com/login/device",
        "https://evil.test/login/device",
        "https://github.com@evil.test/login/device",
    ] {
        assert!(
            device_prompt(
                format!("{code}Open this URL to continue in your web browser: {url}\n").as_bytes(),
                &server
            )
            .is_none()
        );
    }
    let value = device_prompt(
        format!(
            "{code}Open this URL to continue in your web browser: https://github.com/login/device\n"
        )
        .as_bytes(),
        &server,
    )
    .unwrap();
    assert_eq!(value.code, "ABCD-1234");
}
#[test]
fn login_commands_are_noninteractive_and_do_not_upload_ssh_keys_or_tokens() {
    for provider in Provider::ALL {
        let server = Server::parse(provider.default_host()).unwrap();
        let args = login_arguments(provider, &server);
        assert!(args.contains(&"--web"));
        assert!(args.contains(&"--hostname"));
        assert!(!args.contains(&"--token"));
        assert!(!args.contains(&"--insecure-storage"));
        if provider == Provider::GitHub {
            assert!(args.contains(&"--skip-ssh-key"));
        }
    }
}

#[test]
fn oauth_application_ids_accept_only_safe_public_identifiers() {
    assert!(valid_client_id("aBc0123_-"));
    for value in ["", "--option;exec", "bad id", "token\nvalue"] {
        assert!(!valid_client_id(value));
    }
}

#[test]
fn insecure_credential_storage_warning_is_not_silently_reported_as_success() {
    assert!(insecure_storage(&output(
        0,
        "",
        "! Authentication credentials saved in plain text"
    )));
    assert!(insecure_storage(&output(
        0,
        "Stored credentials in plaintext",
        ""
    )));
    assert!(!insecure_storage(&output(
        0,
        "",
        "Stored your credentials in the operating system keyring."
    )));
}
