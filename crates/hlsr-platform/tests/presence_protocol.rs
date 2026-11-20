//! Протокол моста игры против 0.4.7: корпус строк (`tools/reference/platform/presence-protocol.js`)
//! → принято/отклонено и разбор; кодировщики к игре (ответ о Discord, `chat.line`, `emote.*`,
//! `badge.ready`) → строка.

mod support;

use hlsr_platform::game_presence::protocol as p;
use support::{Mismatches, fixture, items, str_of};

#[test]
fn parse_line_like_0_4_7() {
    let fixture = fixture("presence-protocol.json");
    let cases = items(&fixture["parse"]);
    assert!(cases.len() > 200, "корпус: {}", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let line = str_of(&case["line"]);
        let actual = p::outcome_value(&p::parse_line(line));
        mismatches.check(&actual, &case["result"], format!("parseLine({line:?})"));
    }
    mismatches.finish("parseLine");
}

#[test]
fn encoders_like_0_4_9() {
    let fixture = fixture("presence-protocol.json");
    let cases = items(&fixture["encode"]);
    assert!(cases.len() > 15, "кодировщиков: {}", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let name = str_of(&case["fn"]);
        let args = &case["args"];
        let text = |i: usize| str_of(&args[i]);
        let int = |i: usize| u32::try_from(args[i].as_u64().unwrap()).unwrap();
        let actual = match name {
            "encodeDiscordAck" => p::encode_discord_ack(args[0].as_bool().unwrap()),
            "encodeChatLine" => p::encode_chat_line(text(0), text(1), text(2), text(3), text(4)),
            "encodeEmoteBegin" => p::encode_emote_begin(text(0), int(1), int(2), int(3), int(4)),
            "encodeEmoteFrame" => p::encode_emote_frame(text(0), int(1), text(2)),
            "encodeEmoteReady" => p::encode_emote_ready(text(0), int(1), int(2), text(3)),
            "encodeBadgeReady" => p::encode_badge_ready(text(0), int(1), int(2), text(3)),
            "encodeEmoteMiss" => p::encode_emote_miss(text(0)),
            other => panic!("кодировщик {other}"),
        };
        mismatches.check_eq(
            actual,
            str_of(&case["result"]).to_owned(),
            format!("{name}({args})"),
        );
    }
    mismatches.finish("кодировщики");
}
