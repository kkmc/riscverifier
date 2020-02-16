#[macro_use]
extern crate log;
extern crate env_logger;

extern crate clap;
use clap::{App, Arg};

extern crate pest;
#[macro_use]
extern crate pest_derive;

extern crate topological_sort;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

mod cdwarfinterface;
use cdwarfinterface::CDwarfInterface;

mod dwarfreader;
use dwarfreader::DwarfReader;

mod objectdumpreader;
use objectdumpreader::ObjectDumpReader;

mod specreader;
use specreader::SpecReader;

mod translator;
use translator::Translator;

mod uclidinterface;
use uclidinterface::Uclid5Interface;

mod ir;

mod context;

mod utils;

fn main() {
    env_logger::init();
    let matches = App::new("RISCVerifier")
        .version("1.0")
        .author("Kevin Cheang <kcheang@berkeley.edu>")
        .about("Translates RISC-V assembly (support for 64g only) programs into an IR")
        .arg(
            Arg::with_name("binary")
                .short("b")
                .long("binary")
                .help("RISC-V binary file.")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("spec")
                .short("s")
                .long("spec")
                .help("RISC-V specification file.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("output")
                .help("Specify the output path.")
                .short("o")
                .long("output")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("function")
                .help("Specify a function to verify.")
                .short("f")
                .long("function")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("xlen")
                .help("Specify the architecture XLEN.")
                .short("x")
                .long("xlen")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("ignore-funcs")
                .help("Comma separated list of functions to ignore. E.g. \"foo,bar\"")
                .short("i")
                .long("ignore-funcs")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("struct-macros")
                .help("Comma separated list of struct ids to generate operator macros for. E.g. \"enclave\"")
                .short("m")
                .long("struct-macros")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("array-macros")
            .help("Comma separated list of type definitions to generate operator macros for.")
            .short("a")
            .long("array-macros")
            .takes_value(true),
        )
        .get_matches();
    let xlen = utils::dec_str_to_u64(matches.value_of("xlen").unwrap_or("64"))
        .expect("[main] Unable to parse numberic xlen.");
    if xlen != 64 {
        warn!("[main] Non-64 bit XLEN is not yet tested. Use with caution.");
    }
    // Parse function blocks from binary
    let binary_path = matches.value_of("binary").unwrap();
    let binary_paths = vec![String::from(binary_path)]; // FIXME: Handle multiple binaries
    let function_blocks = ObjectDumpReader::get_binary_object_dump(&binary_paths);
    // Get ignored functions
    let ignored_functions = matches
        .value_of("ignore-funcs")
        .map_or(HashSet::new(), |lst| {
            lst.split(",").collect::<HashSet<&str>>()
        });
    let _struct_macro_ids = matches
        .value_of("struct-macros")
        .map_or(HashSet::new(), |lst| {
            lst.split(",").collect::<HashSet<&str>>()
        });
    let _array_macro_ids = matches
        .value_of("array-macros")
        .map_or(HashSet::new(), |lst| {
            lst.split(",").collect::<HashSet<&str>>()
        });
    // Initialize DWARF reader
    let dwarf_reader: Option<DwarfReader<CDwarfInterface>> =
        if matches.value_of("ignore-funcs").is_some()
            || matches.value_of("struct-macros").is_some()
            || matches.value_of("array-macros").is_some()
        {
            DwarfReader::new(&binary_paths).ok()
        } else {
            None
        };
    // Function to generate
    let func_name = matches
        .value_of("function")
        .expect("[main] No function given to translate.");
    // Translate and write to output file
    let mut func_blks = HashMap::new();
    for (k, v) in function_blocks {
        let blk = v.iter().map(|al| Rc::new(al.clone())).collect::<Vec<_>>();
        let cfg = Rc::new(ObjectDumpReader::get_cfg(blk.clone()));
        func_blks.insert(format!("{}", k), Rc::clone(&cfg));
        func_blks.insert(blk[0].function_name().to_string(), Rc::clone(&cfg));
    }
    let mut translator: Translator<Uclid5Interface, CDwarfInterface> =
        Translator::new(&func_blks, &ignored_functions, &dwarf_reader);
    translator.gen_func_model(&func_name);
    translator.print_model();
}
