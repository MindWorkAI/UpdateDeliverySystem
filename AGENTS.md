# Update Delivery System
The Update Delivery System (UDS) is a Rust-based system for globally distributing Tauri updates for MindWork AI Studio. Large organizations can also operate the UDS themselves: This allows them to control how quickly they want to distribute global updates within their organization.

# Endpoints
The UDS has both public and private endpoints, which are secured with a private token.

## Public Endpoints:
API as intended by Tauri v2 in order to distribute updates. We want to support different channels, such as `stable`, `beta`, `experimental`, or `lts`.

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
