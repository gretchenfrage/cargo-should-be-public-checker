# cargo should-be-public-checker

Cargo utility to detect types and traits which are exposed in a project's public API, but not
importable through the project directly. This includes both items within the same crate, and items
within external crates. Such problems can be fixed by adding `pub use` or `pub extern crate`
statements.

Run this command as:

```sh
should-be-public-checker
should-be-public-checker path/to/project
should-be-public-checker path/to/project -p name-of-package
```

Outputs something like:

```
visible but not importable:
- quinn::ClientConfig::initial_dst_cid_provider::ConnectionId
- quinn::ClientConfig::initial_dst_cid_provider::ConnectionId::from_buf::Buf
- quinn::Connecting::`<_ as VZip<Some(AngleBracketed { args: [Type(Generic("V"))], constraints: [] })>>`::MultiLane
- quinn::ConnectionClose::error_code::Code
- quinn::ConnectionClose::frame_type::Type
- quinn::ConnectionClose::reason::Bytes
- quinn::ConnectionStats::frame_tx::FrameStats
- quinn::ConnectionStats::path::PathStats
- quinn::ConnectionStats::udp_tx::UdpStats
- quinn::EndpointConfig::cid_generator::ConnectionIdGenerator
- quinn::SendStream::write_chunks::Written
- quinn::StreamId::new::Dir
- quinn::StreamId::new::Side
- quinn::Transmit::ecn::EcnCodepoint
- quinn::VarInt::from_u64::VarIntBoundsExceeded
```

This project is still in a relatively crude state. However, it does already work well enough to
produce useful results, although the output requires manual inspection and may still contain both
false positives and false negatives, and may not be able to handle certain dependency graphs and
other cases.

Utilitizes `cargo doc`'s experimental JSON output feature to work. The creation of this project was
inspired by https://github.com/quinn-rs/quinn/issues/2012.
