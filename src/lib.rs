use std::collections::HashMap;
use std::sync::RwLock;

pub mod atomic;
pub mod model;


#[cfg(test)]
mod tests {
    use crate::model::Str;

    #[test]
    fn it_works() {
        let q = size_of::<Str>();
        println!("{}", q);
    }
}
