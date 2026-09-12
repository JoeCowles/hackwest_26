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

Section 14 contains the broader web/monitoring design. The user subsequently
authorized live frontend integration: section 16 records the implemented read
subset, deployment, and explicit deviations/remaining work. Section 15 describes
the core collector API. Do not claim still-planned monitoring or alert features
are implemented. Web assets are embedded at compile time and served same-origin.

Keep heartbeats at 5 seconds. Preserve observation states, stable object
identities, boot/generation boundaries, and idempotent batch semantics.
Never treat missing telemetry as zero or sum shared APFS/NFS capacity twice.
Do not add node collection code to the central server.

Run cargo test --workspace and cargo check --workspace for substantive server
changes. The desktop feature is default; --no-default-features builds headless.
