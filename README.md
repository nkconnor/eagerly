# eagerly

Views asyncronously retrieved values and refreshes them in the background
at a pre-determined interval. The view is backed by [ArcSwap](https://docs.rs/arc-swap/0.4.7/arc_swap/)
which provides fast, lock-free reads.

## Example Usage

```rust
  let user_ids = View<Vec<u32>> =
    view(|| async {
        let user_ids = database_call().await();
        //
    })
      .frequency(Duration::from_secs(60 * 3))
      .load()
      .await;
```

[View](struct.Cache.html) is thread-safe and implements [Clone](std::marker::Clone), which provides a
replica pointing to the same underlying storage.

```rust
std::thread::spawn(|| {
    let user_ids = user_ids.clone();
    // ^ receives same updates
});
```
