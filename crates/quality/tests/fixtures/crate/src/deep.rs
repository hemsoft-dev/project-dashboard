pub mod inner;

pub trait Named {
    fn name(&self) -> &str {
        "deep"
    }
    fn required(&self);
}
