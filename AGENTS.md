# Update Delivery System
The Update Delivery System (UDS) is a Rust-based system for globally distributing Tauri updates for MindWork AI Studio. Large organizations can also operate the UDS themselves: This allows them to control how quickly they want to distribute global updates within their organization.

# Endpoints
The UDS has both public and private endpoints, which are secured with a private token.

## Public Endpoints:
API as intended by Tauri v2 in order to distribute updates. We want to support different channels, such as `stable`, `beta`, `experimental`, or `mature`. The `mature` channel contains older, field-tested releases for environments that prioritize reliability over receiving updates quickly; it does not imply long-term support or an extended maintenance commitment.

## Private Admin Endpoints with Secret Token
- Upload a new release for a channel. Due to load balancing, only one UDS node will receive this call. It must then communicate with all others in the private network (broadcast?) and replicate the update to all nodes.
- Withdraw a release. This call must also be replicated to all nodes.
- Copy an existing release into another channel.
- Retrieve statistics for a channel with a call: number of requests, number of downloads per architecture (x86 vs ARM) and OS (macOS, Linux, Windows), estimated total traffic. The call should return the statistics for the entire fleet of n nodes. To do this, it must query all nodes, e.g. via broadcast. Each node manages its own statistics locally. We want to work directly with the file system here, for example by storing a file with a UUID as the name for each download, so that we can handle downloads without collisions and without locks. A background thread can count the UUIDs every 15 minutes and increment a counter. The counter is persisted in a text file. We do the same with the other statistics.

# Architecture
n instances of the UDS are started as UDS nodes. All nodes are located in a private network. There is a public load balancer that distributes each request to one of the n nodes. This has consequences: admin calls only arrive at one of the n nodes and must then be replicated to the others.

In addition, new nodes should configure themselves: via broadcast, the UDS fleet should find itself and organize itself independently.

# Software
We use the latest Rust version with the latest Edition 2024. For the HTTPS API, we use `axum`.

# Changelog and releases
Document every user-facing change directly in `CHANGELOG.md` under the current version. Each entry starts with a dash and a space (`- `) and one of the following words:

- Added
- Released
- Improved
- Changed
- Fixed
- Updated
- Removed
- Downgraded
- Upgraded

The entire release changelog is sorted by these categories in the order shown above. The language used for the changelog is US English.

Each new release gets a new `# UDS v<SemVer>` heading (newest first), and both the package version and UDS build number in `Cargo.toml` must be incremented together.

# Source code readability
UDS source code is maintained for people first, including developers who are new to Rust. Keep the following rules when adding or changing code:

- Write comments and Rust documentation in US English.
- Separate functions and distinct logical steps with blank lines. Do not pack unrelated operations together merely to reduce the line count.
- Add a short `//` comment before a complex or non-obvious operation. Explain the intent, relevant fallback, or UDS-specific reason instead of translating the Rust syntax word for word.
- Introduce a longer logical section with this exact visual pattern:

  ```rust
  //
  // Describe the purpose of the following block.
  //
  ```

- Give every struct and enum, including private request and state types, a `///` comment that explains what it represents and why UDS needs it.
- Document fields and enum variants when their meaning, unit, source, security impact, or lifecycle is not immediately clear.
- Document every function and method with `///`, regardless of visibility. This includes test functions, test helpers, and standard trait implementations. Explain the UDS-specific purpose, behavior, or guarantee instead of merely repeating the function name.
- Document public and private modules, constants, fields, and enum variants as well. The Rust `missing_docs` lint covers the public API and Clippy's `missing_docs_in_private_items` covers internal code; both are required at deny level and must pass for every target.
- Treat a documented or annotated struct field or enum variant as one visual block: keep its documentation, attributes, and declaration together, and separate it from the next block with a blank line. Undocumented and unannotated fields may remain compact.
- Start every non-trivial module with a `//!` comment describing its responsibility and its place in UDS.
- Prefer named intermediate values over deeply nested expressions when the names make the data flow easier to follow.
- Keep each module focused on one responsibility. Treat roughly 500 lines of production code as a prompt to look for a useful responsibility boundary, not as a mechanical limit.
- Keep `src/main.rs` limited to the Tokio entry point and application startup. Put command dispatch, server lifecycle, and shutdown behavior in dedicated modules.
- Give each interactive client prompt use case its own module. Keep prompt dispatch and the main menu in `client::prompts`, and put shared selection or profile helpers in focused support code.

# Rust formatting
Run `cargo fmt` for the normal Rust layout, using the repository's `rustfmt.toml`. Human-readable layout takes precedence where rustfmt cannot express the intent:

- Keep each Axum route registration on one line, even when it exceeds the normal width. This makes route tables scannable.
- Use `#[rustfmt::skip]` only on the smallest item that requires a deliberate manual layout. Never skip an entire file or module.
- Do not use a formatter exception to hide generally dense code. First try blank lines, named intermediate values, or a smaller helper function.
- After editing a skipped item, preserve its manual layout and verify it during review.
