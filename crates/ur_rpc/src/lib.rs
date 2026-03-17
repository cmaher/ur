#[cfg(feature = "error")]
pub mod error;

#[cfg(feature = "stream")]
pub mod stream;

pub mod proto {
    // Generated tonic code triggers excessive_nesting in deeply nested impl blocks.
    #[allow(clippy::excessive_nesting)]
    pub mod core {
        tonic::include_proto!("ur.core");
    }
    #[cfg(feature = "hostexec")]
    #[allow(clippy::excessive_nesting)]
    pub mod hostexec {
        tonic::include_proto!("ur.hostexec");
    }
    #[cfg(feature = "builder")]
    #[allow(clippy::excessive_nesting)]
    pub mod builder {
        tonic::include_proto!("ur.builder");
    }
    #[cfg(feature = "rag")]
    #[allow(clippy::excessive_nesting)]
    pub mod rag {
        tonic::include_proto!("ur.rag");
    }
    #[cfg(feature = "ticket")]
    #[allow(clippy::excessive_nesting)]
    pub mod ticket {
        tonic::include_proto!("ur.ticket");
    }
    #[cfg(feature = "workerd")]
    #[allow(clippy::excessive_nesting)]
    pub mod workerd {
        tonic::include_proto!("ur.workerd");
    }
}
