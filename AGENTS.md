# Orchard server development

The project uses Rust. The working checkout is on the Desktop; some older task
contexts still refer to Documents/ChatGPT/hackwest_26.

The shared source of truth is the Server Spec tab:
https://docs.google.com/document/d/1JnsZHlYPXQ1IRsMSEHeboICzqqviKJylI7gRsuUK13E/edit?tab=t.bclfm0yxwd6r

When adding or changing an API, update that Google Doc during the same work:
document the full route, authentication, request/response fields, status codes,
examples, and implementation/verification status. Use native Google Docs
formatting. A local Markdown file is not a substitute for the shared spec.
Do not claim the documentation was updated unless the connector write succeeded.

Section 14 contains planned web/monitoring contracts explicitly deferred by the
user. Do not implement these routes or mark them available incidentally while
working on ingestion. Section 15 describes the implemented core server API.

Keep heartbeats at 5 seconds. Preserve observation states, stable object
identities, boot/generation boundaries, and idempotent batch semantics.
Never treat missing telemetry as zero or sum shared APFS/NFS capacity twice.
Do not add node collection code to the central server.

Run cargo test --workspace and cargo check --workspace for substantive server
changes. The desktop feature is default; --no-default-features builds headless.
