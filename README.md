# eagerly

Caches values in advance and asyncronously refreshes the value
at a pre-determined frequency. The cache is backed by [ArcSwap](https://docs.rs/arc-swap/0.4.7/arc_swap/)
which provides fast, lock-free reads.

## Example Usage

```rust
  let user_ids = Cache<Vec<u32>, _> =
    cache(|| async {
        vec![1,2,3] // expensive database call happens here
    })
      .frequency(Duration::from_secs(60 * 3))
      .load()
      .await;

  assert_eq!(**user_ids.read(), vec![1,2,3])
```

