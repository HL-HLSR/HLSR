//! `redactText` из `diagnostics/redact.js` 0.4.7: без имени пользователя и с ним (`userName`), `safeEnv`.

use hlsr_core::redact::{ALLOWED_ENV, is_secret_env_name, redact_text, redact_text_for, safe_env};

fn check(cases: &[(&str, &str)]) {
    for (input, expected) in cases {
        assert_eq!(redact_text(input), *expected, "вход: {input:?}");
    }
}

#[test]
fn secrets() {
    check(&[
        (
            "a -----BEGIN RSA PRIVATE KEY-----\nMII\n-----END RSA PRIVATE KEY----- b",
            "a -----BEGIN PRIVATE KEY----- <redacted> -----END PRIVATE KEY----- b",
        ),
        (
            "Authorization: Bearer abc\nnext",
            "Authorization: <redacted>\nnext",
        ),
        ("cookie :x=1; y=2", "cookie: <redacted>"),
        (
            "Proxy-Authorization:basic",
            "Proxy-Authorization: <redacted>",
        ),
        ("use Bearer abc.def== now", "use Bearer <redacted> now"),
        // Bearer — с учётом регистра.
        ("bearer abc", "bearer abc"),
        ("password: \"hunter2\" ok", "password: <redacted> ok"),
        ("api-key=abc;x", "api-key=<redacted>;x"),
        ("TOKEN = abc", "TOKEN = <redacted>"),
        ("tokens=abc", "tokens=abc"),
        // Кавычка без пары: правило не срабатывает.
        ("token=\"abc def\"", "token=\"abc def\""),
        ("x ghp_ABCDEFGHIJKLMNOP1 y", "x <redacted> y"),
        ("ghp_ABCDEFGHIJKLMNOP_x", "ghp_ABCDEFGHIJKLMNOP_x"),
        (
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI k",
            "ssh-key <redacted> k",
        ),
    ]);
}

#[test]
fn urls_identifiers_and_paths() {
    check(&[
        (
            "get https://x.io/a?sig=1&b=2 next",
            "get https://x.io/a?<redacted> next",
        ),
        ("https://x.io/a?page=2&sig", "https://x.io/a?page=2&sig"),
        (
            "HTTPS://X.IO/?X-Amz-Signature=1",
            "HTTPS://X.IO/?<redacted>",
        ),
        ("(https://x.io/?st=1)", "(https://x.io/?<redacted>)"),
        ("mail me: a.b+c@mail.example.org.", "mail me: <email>."),
        ("id 76561198000000000 x", "id <steamid> x"),
        ("id 765611980000000001", "id 765611980000000001"),
        ("c:\\users\\Bob\\x", "c:\\users\\<user>\\x"),
        ("C:\\Users\\Иван\\HLSR", "C:\\Users\\<user>\\HLSR"),
        ("/Users/bob/x", "/Users/<user>/x"),
        ("/home/bob/x", "/home/bob/x"),
        // Сначала имя с @ становится <email>, затем правило путей — <user>.
        ("C:\\Users\\a@b.cc\\x", "C:\\Users\\<user>\\x"),
    ]);
}

#[test]
fn javascript_regex_semantics() {
    check(&[
        // \b в JS — только ASCII: кириллица перед ключом — граница слова.
        ("яtoken=1", "яtoken=<redacted>"),
        // /i без флага u не сводит ſ (U+017F) к s и K (U+212A) к k.
        ("ſecret=1", "ſecret=1"),
        ("api\u{212A}ey=1", "api\u{212A}ey=1"),
        // \s в JS: есть U+FEFF, нет U+0085.
        ("token=a\u{FEFF}b", "token=<redacted>\u{FEFF}b"),
        ("token=a\u{85}b", "token=<redacted>"),
        // \d в JS — только ASCII.
        ("76561198000000000\u{0661}", "<steamid>\u{0661}"),
        ("7656119800000000\u{0661}0", "7656119800000000\u{0661}0"),
    ]);
}

#[test]
fn user_name_is_hidden_anywhere_without_regard_to_case() {
    let cases = [
        (
            "/home/alice/Games err: /home/Alice/x",
            "alice",
            "/home/<user>/Games err: /home/<user>/x",
        ),
        (
            "ALICE Alice alice malice",
            "alice",
            "<user> <user> <user> m<user>",
        ),
        (
            r"C:\Users\Иван\x ИВАН иван",
            "Иван",
            r"C:\Users\<user>\x <user> <user>",
        ),
        ("a.b axb", "a.b", "<user> axb"),
        ("c++c++", "c++", "<user><user>"),
        ("cost $& $1", "$&", "cost <user> $1"),
        // Имя заменяется раньше правил путей: «user» находится и внутри «Users».
        (r"C:\Users\bob\x", "user", r"C:\<user>s\bob\x"),
        // /i без u: ß не равно SS, ſ не равно s, знак Кельвина не равен k.
        ("SS ss", "ß", "SS ss"),
        ("\u{17F}", "s", "\u{17F}"),
        ("\u{212A}", "k", "\u{212A}"),
        ("😀alice😀", "😀", "<user>alice<user>"),
    ];
    for (text, name, expected) in cases {
        assert_eq!(
            redact_text_for(text, Some(name)),
            expected,
            "{text:?} / {name:?}"
        );
    }
    // Пустое имя и его отсутствие ничего не меняют.
    assert_eq!(redact_text_for("alice", Some("")), "alice");
    assert_eq!(redact_text_for("alice", None), redact_text("alice"));
}

#[test]
fn safe_env_keeps_only_allowed_plain_names() {
    let env: Vec<(String, String)> = [
        ("DISPLAY", ":0"),
        ("XDG_SESSION_TYPE", "wayland"),
        ("COMPUTERNAME", "ALICE-PC"),
        ("HLSR_TOKEN", "secret"),
        ("OS", "https://x.io/?sig=1"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    let out = safe_env(&env, None, Some("alice"));
    assert_eq!(
        out,
        vec![
            ("OS".to_owned(), "https://x.io/?<redacted>".to_owned()),
            ("COMPUTERNAME".to_owned(), "<user>-PC".to_owned()),
            ("DISPLAY".to_owned(), ":0".to_owned()),
        ]
    );
    // Порядок — как в списке допустимых; секретное имя из списка всё равно отбрасывается.
    assert_eq!(ALLOWED_ENV[0], "NUMBER_OF_PROCESSORS");
    let custom = safe_env(&env, Some(&["DISPLAY", "HLSR_TOKEN", "NOPE"]), None);
    assert_eq!(custom, vec![("DISPLAY".to_owned(), ":0".to_owned())]);
    for secret in [
        "TOKEN",
        "my_key",
        "KEY_LAYOUT",
        "author",
        "XDG_SESSION_TYPE",
        "pwd",
    ] {
        assert!(is_secret_env_name(secret), "{secret}");
    }
    for plain in ["DISPLAY", "MONKEY", "KEYBOARD", "OS", "TO\u{212A}EN"] {
        assert!(!is_secret_env_name(plain), "{plain}");
    }
}
