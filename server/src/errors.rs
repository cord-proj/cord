use crate::message::Message;
use error_chain::*;

error_chain! {
    foreign_links {
        Io(::std::io::Error);
        TokioChan(::tokio::sync::mpsc::error::UnboundedTrySendError<Message>);
        TokioExec(::tokio::executor::SpawnError);
    }

    errors {
        OversizedNamespace {
            description("Namespace length cannot be greater than a u16")
            display("Namespace length cannot be greater than a u16")
        }
    }
}
