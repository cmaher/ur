pub mod proto {
    // Generated tonic code triggers excessive_nesting in deeply nested impl blocks.
    #[allow(clippy::excessive_nesting)]
    pub mod core {
        tonic::include_proto!("ur.core");
    }
    #[cfg(feature = "git")]
    #[allow(clippy::excessive_nesting)]
    pub mod git {
        tonic::include_proto!("ur.git");
    }
    #[cfg(feature = "gh")]
    #[allow(clippy::excessive_nesting)]
    pub mod gh {
        tonic::include_proto!("ur.gh");
    }
}
