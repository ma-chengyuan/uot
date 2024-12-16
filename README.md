# Socks-UoT: Full Cone UDP for TCP-only proxy systems

Socks-UoT is a simple program that aims to do just one simple thing right: bringing UDP support for proxy systems that originally only supports TCP or whose UDP support is incomplete.

## Background

Various proxy programs are used to bypass firewalls or circumvent Internet censorship. While the underlying protocols are rarely SOCKS5 itself (due to its insecurity and detectability), such programs usually expose its inbound as a local SOCKS5 server which most client programs know how to properly handle. 

SOCKS5, as defined in RFC1928, supports proxying both TCP and UDP traffic, but unfortunately many proxy systems are TCP-only, so when a client sends a UDP ASSOCIATE request to their inbound it gets a Command Not Supported error in response. Some proxy protocols claim to support UDP, but its implementation could be faulty, i.e. the UDP association could be *symmetric* instead of *full-cone*, causing unexpected behaviors for UDP clients.

Some notable protocols that does not fully support UDP:
* VMess (only symmetric NAT, see [here](https://github.com/XTLS/Xray-core/discussions/237))
* VLESS (only symmetric NAT, see [here](https://github.com/XTLS/Xray-core/discussions/237))
* NaïveProxy (the author explicitly says that UDP support is not on the roadmap, see [here](https://github.com/klzgrad/naiveproxy/issues/234)) 

While indeed proxying just TCP covers nearly every real-world use cases, a UDP-aware proxy is often desirable for scenarios such as

* DNS query (in its simplest form),
* video conferencing,
* livestreaming,
* HTTP/3 (QUIC),
* some gaming applications,
* etc.

In addition, if one intends to use techniques such as Tun2socks to emulate a "global proxy," a UDP-aware proxy provides a more polished experience. It also helps prevents DNS pollutions, as UDP-based DNS queries wouldn't be proxied otherwise.

Socks-UoT is a thin wrapper that aims to bring full cone UDP support to these protocols, by sending UDP packets over a TCP connection established by the downstream proxy and re-sending them as UDP on the server side.

## Building and Running

To build, run `cargo build` as one would for any Rust project.

### Running the Server

To run the server, run 
```bash
./server --local <listen address>
# or cargo run --bin server -- --local <listen address>
# if you want to build and run from source
```
For example,
```
./server --local 0.0.0.0:39999
```

### Running the Client

Suppose the TCP-only proxy exposes its SOCKS5 inbound at localhost:1080, the server is open at 1.2.3.4:39999, and you would like the client to expose its SOCKS5 inbound at localhost:1081, then run
```bash
./client --local localhost:1081 --server 1.2.3.4:39999 --remote localhost:1080
# or cargo run --bin client -- ...
# if you want to build and run from source
```

Note that in this scenario, although technically you should be able to set the server listen address to localhost:39999 and write `--server localhost:39999` in your client arguments (because this address will be resolved on the server side anyways), you are advised to use an explicit IP if possible, because some downstream proxy clients could handle requests to localhost specially without notice.

