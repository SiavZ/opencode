//! Black-box contract for the standalone launcher and its real interpreter stdio ABI.
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn request(
    input: &mut impl Write,
    output: &mut impl BufRead,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    )
    .unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    assert!(
        output.read_line(&mut line).unwrap() > 0,
        "bridge exited before replying"
    );
    let response: Value =
        serde_json::from_str(line.strip_prefix('\u{1e}').expect("framed bridge reply")).unwrap();
    assert_eq!(response["id"], id);
    response
}

#[test]
fn launcher_runs_real_node_plugin_and_exits_on_shutdown() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opencode-compatibility-host"))
        .arg("node")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let response = request(
        &mut input,
        &mut output,
        1,
        "initialize",
        json!({"abiVersion":1}),
    );
    assert_eq!(response["result"]["runtime"], "node");
    assert_eq!(response["result"]["abiVersion"], 1);
    let response = request(
        &mut input,
        &mut output,
        2,
        "module.import",
        json!({"entry":"data:text/javascript,export default async function(){return {'chat.params':async(i,o)=>{o.temperature=0.25}}}"}),
    );
    let module = response["result"]["module"].clone();
    let response = request(
        &mut input,
        &mut output,
        3,
        "plugin.init",
        json!({"module":module,"export":"default","member":"self","input":{"directory":"/","worktree":"/","project":{},"serverUrl":"http://127.0.0.1:9"}}),
    );
    assert_eq!(
        response["result"]["hooks"]["triggers"],
        json!(["chat.params"])
    );
    let handle = response["result"]["handle"].clone();
    let response = request(
        &mut input,
        &mut output,
        4,
        "hook.trigger",
        json!({"handle":handle,"name":"chat.params","input":{},"output":{"temperature":1}}),
    );
    assert_eq!(response["result"]["output"]["temperature"], 0.25);
    let response = request(
        &mut input,
        &mut output,
        5,
        "initialize",
        json!({"abiVersion":999}),
    );
    assert_eq!(response["error"]["code"], 1004);
    let response = request(&mut input, &mut output, 6, "shutdown", json!({}));
    assert!(response["result"].is_null());
    assert!(child.wait().unwrap().success());
}

#[test]
fn launcher_rejects_missing_extra_and_unavailable_interpreters() {
    for args in [
        vec![],
        vec!["node", "extra"],
        vec!["/nonexistent/opencode-interpreter"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_opencode-compatibility-host"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
