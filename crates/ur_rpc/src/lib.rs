#[cfg(feature = "error")]
pub mod error;

pub mod lifecycle;

#[cfg(feature = "stream")]
pub mod stream;

#[cfg(feature = "stream")]
mod builderd;

pub mod proto {
    // Generated tonic code triggers excessive_nesting in deeply nested impl blocks.
    #[allow(clippy::excessive_nesting)]
    pub mod core {
        tonic::include_proto!("ur.core");
    }
    #[allow(clippy::excessive_nesting)]
    pub mod hostexec {
        tonic::include_proto!("ur.hostexec");
    }
    #[allow(clippy::excessive_nesting)]
    pub mod builder {
        tonic::include_proto!("ur.builder");

        /// Pre-connected builderd gRPC client (cheap to clone).
        pub type BuilderdClient =
            builder_daemon_service_client::BuilderDaemonServiceClient<tonic::transport::Channel>;
    }
    #[allow(clippy::excessive_nesting)]
    pub mod rag {
        tonic::include_proto!("ur.rag");
    }
    #[allow(clippy::excessive_nesting)]
    pub mod ticket {
        tonic::include_proto!("ur.ticket");
    }
    #[allow(clippy::excessive_nesting)]
    pub mod workerd {
        tonic::include_proto!("ur.workerd");
    }
    #[allow(clippy::excessive_nesting)]
    pub mod remote_repo {
        tonic::include_proto!("ur.remote_repo");
    }
}
