# ats/data

Analysis, Transformation and Synthesis (ATS) in Pure Data.

Some years ago I wrote an object for PD called `atsread` that simply read out data from an ATS file.

This project is a new approach for using ATS in [Pure Data (aka PD)](https://puredata.info/).

## To build

This project is programmed in [rust](https://www.rust-lang.org/).  I recommend that you use [rustup](https://rustup.rs/) to install rust.

Once you have rust installed, you'll need [cargo-make](https://sagiegurari.github.io/cargo-make/).

```
cargo install --force cargo-make'
```

Finally, build the project with:

```
cargo make
```

### To run

Currently only on Mac or Linux, assuming pure data is already installed.

```
cargo make run --profile=release
```


## TODO

* Visualization
	* ticks for the time and frequency axis
* more transformation examples
* analysis examples


## Reference

* [ATS](https://dxarts.washington.edu/wiki/analysis-transformation-and-synthesis-ats)
* [ATS-PD by Pablo Di Liscia](https://github.com/odiliscia/ats-pd_gh) - some externals for reading and synthesizing ATS data.
	Provided good reference for this work. atsread in this project is actually an update of something I wrote a long time ago.
* [Pure Data Rust](https://github.com/x37v/puredata-rust) - bindings to build pure data externals in [Rust](https://www.rust-lang.org/).
