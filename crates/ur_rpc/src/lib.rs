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
    #[cfg(feature = "hostd")]
    #[allow(clippy::excessive_nesting)]
    pub mod hostd {
        tonic::include_proto!("ur.hostd");
    }
    #[cfg(feature = "rag")]
    #[allow(clippy::excessive_nesting)]
    pub mod rag {
        tonic::include_proto!("ur.rag");
    }
}
