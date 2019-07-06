use crate::message::Message;
use error_chain::*;

error_chain! {
    foreign_links {
        Io(::std::io::Error);
        ChanRecv(::tokio::sync::mpsc::error::UnboundedRecvError);
        ChanSend(::tokio::sync::mpsc::error::UnboundedTrySendError<Message>);
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
