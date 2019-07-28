use error_chain::*;
use message::Message;
use tokio::sync::mpsc::error::{RecvError, UnboundedRecvError, UnboundedTrySendError};

error_chain! {
    foreign_links {
        ConnRecv(UnboundedRecvError);
        ConnSend(UnboundedTrySendError<Message>);
        Io(::std::io::Error);
        Message(message::errors::Error);
        SubscriberError(RecvError);
    }
}
