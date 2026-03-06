# ur_rpc (Shared RPC Contract)

Shared between all crates. Changes here affect `ur`, `urd`, and `agent_tools`.

- All request/response types must derive `Serialize, Deserialize, Debug, Clone`
- Adding a method to `UrAgentBridge` requires updating: `urd/src/main.rs` (impl), `urd/tests/bridge_test.rs` (stub impl), and the relevant client crate
- Uses `Result<T, String>` for trait methods (tarpc constraint) — convert `anyhow::Error` via `.map_err(|e| e.to_string())` at the boundary
