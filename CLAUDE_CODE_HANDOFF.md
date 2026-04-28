# First Prompt for Claude Code

When you start Claude Code in this directory, hand it this prompt verbatim:

---

I'm building cbootc, a minimal bootc-like tool for systems running on
composefs-rs. Read DESIGN.md for the full design rationale, then look at
README.md, the example Containerfiles under examples/, and the disk-builder
script under tools/.

Start with step 1 of the implementation plan in DESIGN.md: create a Rust
project skeleton with `cargo init`, set up clap for CLI parsing, and stub
out the five commands (upgrade, status, rollback, switch, verify) so they
each print "not implemented" and exit cleanly. Use clap's derive API.

Pin recent versions of clap and anyhow. No async runtime yet — none of the
v1 commands need it.

Once the skeleton compiles and `cargo run -- --help` works, we'll move to
step 2 (implementing `cbootc status` for real).

Don't add features beyond what DESIGN.md scopes. If something feels like it
belongs but isn't in DESIGN.md, ask before adding it — the tool's small size
is a feature, not an oversight.

---

That's it. From there the conversation continues naturally.
