pub fn hello() -> &'static str {
    "ur_rpc"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        assert_eq!(hello(), "ur_rpc");
    }
}
