pub fn leaf(x: i32) -> i32 {
    x + 1
}

pub fn middle(x: i32) -> i32 {
    leaf(x) + leaf(x + 1)
}

pub fn root(x: i32) -> i32 {
    middle(x)
}

// An anonymous closure: must not appear as a named callable.
pub fn with_closure(v: &[i32]) -> Vec<i32> {
    v.iter().map(|n| n * 2).collect()
}
