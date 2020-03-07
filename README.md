# atsdata

Analysis, Transformation and Synthesis (ATS) in Pure Data.

This project is a new approach for using ATS in [Pure Data (aka PD)](https://puredata.info/).

Some years ago I wrote an object for PD called `atsread` that simply read out data from an ATS file.
This project uses PD's `structs` to store data that can be visualized as well as used for synthesis.

It is a work in progress!

```
cargo make run --profile=release
```


## TODO

* Better Visualization
	* ticks for the time and frequency axis
* Analysis of data in tables
* remove clicking if possible?
* more transformation examples
* analysis examples


## Reference

* [ATS](https://dxarts.washington.edu/wiki/analysis-transformation-and-synthesis-ats)
* [ATS-PD by Pablo Di Liscia](https://github.com/odiliscia/ats-pd_gh) - some externals for reading and synthesizing ATS data.
	Provided good reference for this work. atsread in this project is actually an update of something I wrote a long time ago.
* [Pure Data Rust](https://github.com/x37v/puredata-rust) - bindings to build pure data externals in [Rust](https://www.rust-lang.org/).
