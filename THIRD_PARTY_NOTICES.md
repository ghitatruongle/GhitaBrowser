# Third-party notices

GhitaBrowser's original code is proprietary. The build uses separately
licensed dependencies; this notice does not change their licenses.

## Direct Rust dependencies

| Package | Locked version | Declared license |
| --- | --- | --- |
| anyhow | 1.0.104 | MIT OR Apache-2.0 |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| cosmic-text | 0.10.0 | MIT OR Apache-2.0 |
| dirs | 5.0.1 | MIT OR Apache-2.0 |
| encoding_rs | 0.8.35 | (Apache-2.0 OR MIT) AND BSD-3-Clause |
| env_logger | 0.11.11 | MIT OR Apache-2.0 |
| ed25519-dalek | 2.2.0 | BSD-3-Clause |
| flate2 | 1.1.9 | MIT OR Apache-2.0 |
| fs2 | 0.4.3 | MIT OR Apache-2.0 |
| iced | 0.12.1 | MIT |
| iced_core | 0.12.3 | MIT |
| image | 0.24.9 | MIT OR Apache-2.0 |
| log | 0.4.33 | MIT OR Apache-2.0 |
| reqwest | 0.13.4 | MIT OR Apache-2.0 |
| semver | 1.0.28 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| tokio | 1.53.1 | MIT |
| tungstenite | 0.30.0 | MIT OR Apache-2.0 |
| ureq | 2.12.1 | MIT OR Apache-2.0 |
| url | 2.5.8 | MIT OR Apache-2.0 |
| windows | 0.52.0 | MIT OR Apache-2.0 |
| winres | 0.1.12 | MIT |
| bytemuck (Windows) | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| pollster (Windows) | 1.0.1 | Apache-2.0 OR MIT |
| wgpu (Windows) | 0.19.4 | MIT OR Apache-2.0 |

Development-only dependencies include Criterion 0.5.1 and rstest 0.18.2,
both declared Apache-2.0 OR MIT. The complete transitive inventory and exact
versions are recorded by `Cargo.lock`; CI rejects dependencies with missing
license metadata and flags copyleft-only expressions for review.

Binary distributions must include this file and the project license. Before a
commercial release, counsel should review the generated locked graph and all
attribution requirements; this inventory is not legal advice.
