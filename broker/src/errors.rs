use error_chain::*;
use message;

error_chain! {
    foreign_links {
        ChanRecv(::tokio::sync::mpsc::error::UnboundedRecvError);
        ChanSend(::tokio::sync::mpsc::error::UnboundedTrySendError<message::Message>);
        Io(::std::io::Error);
        Message(message::errors::Error);
        TokioExec(::tokio::executor::SpawnError);
    }

    errors {
        OversizedData {
            description("Data length cannot be greater than a u32")
            display("Data length cannot be greater than a u32")
        }

        OversizedNamespace {
            description("Namespace length cannot be greater than a u16")
            display("Namespace length cannot be greater than a u16")
        }
    }
}
