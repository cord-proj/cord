use error_chain::*;

error_chain! {
    foreign_links {
        Io(::std::io::Error);
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
