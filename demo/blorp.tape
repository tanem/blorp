# Renders demo/blorp.gif. Run `vhs demo/blorp.tape` from the repo root.
#
# blorp runs as a debug build against the made-up account that
# examples/demo_server.rs serves on localhost, never a real account. The
# token and log stay out of the keyring and the home directory.

Output demo/blorp.gif

Require cargo
Require curl

Set Shell "bash"
Set FontSize 18
Set Width 1100
Set Height 520
Set Padding 20
Set TypingSpeed 100ms

Hide
Type "export PS1='$ ' BLORP_DEMO=$(mktemp -d)" Enter
Type "cargo build -q --bin blorp --example demo_server" Enter
Wait@300s /\$$/
Type "target/debug/examples/demo_server > /dev/null &" Enter
Type "until curl -so /dev/null http://127.0.0.1:4848/; do sleep 0.1; done" Enter
Wait /\$$/
Type "export PATH=$PWD/target/debug:$PATH XDG_STATE_HOME=$BLORP_DEMO BLORP_API_TOKEN=demo-token BLORP_SESSION_URL=http://127.0.0.1:4848/jmap/session" Enter
Type "clear" Enter
Wait /\$$/
Show

Sleep 500ms
Type "blorp"
Sleep 500ms
Enter
Sleep 2s
Type "j"
Sleep 700ms
Type "j"
Sleep 700ms
Type "j"
Sleep 700ms
Type "j"
Sleep 1s
Type "k"
Sleep 700ms
Type "k"
Sleep 700ms
Type "k"
Sleep 1s
Type "r"
Sleep 1.5s
Type "j"
Sleep 1s
Type "e"
Sleep 1.5s
Type "u"
Sleep 2s
Type "u"
Sleep 1.5s
Type "m"
Sleep 1s
Type "arch"
Sleep 700ms
Enter
Sleep 2s
Type "q"
Sleep 1s

Hide
Type "kill %1; rm -rf $BLORP_DEMO" Enter
