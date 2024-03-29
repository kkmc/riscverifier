#[macro_use]
extern crate log;
extern crate env_logger;

extern crate clap;
use clap::{App, Arg};

extern crate pest;
#[macro_use]
extern crate pest_derive;

extern crate asts;
extern crate dwarf_ctx;
extern crate rv_model;
extern crate utils;

extern crate topological_sort;

pub mod disassembler;
use disassembler::disassembler::Disassembler;

pub mod translator;
use translator::Translator;

pub mod verification_interfaces;
use verification_interfaces::uclidinterface::Uclid5Interface;

pub mod datastructures;
use datastructures::cfg::BasicBlock;

pub mod spec_template_generator;
use spec_template_generator::SpecTemplateGenerator;

pub mod vectre_program_generator;
use vectre_program_generator::VectreProgramGenerator;

pub mod ir_interface;

// pub mod utils;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::File,
    io::prelude::*,
    rc::Rc,
};

use dwarf_ctx::{
    dwarf_interfaces::cdwarfinterface::CDwarfInterface,
    dwarfreader::{DwarfCtx, DwarfReader, DwarfTypeDefn},
};

use asts::spec_lang::{sl_ast, sl_ast::ASTRewriter, sl_parser};

use rv_model::system_model;

use utils::{constants, helpers};

// ================================================================================================
/// # RICS-V Translator Main Function

/// Process the commands given to the tool
pub fn process_commands() {
    let matches = cl_options().get_matches();
    let xlen = helpers::dec_str_to_u64(matches.value_of("xlen").unwrap_or("64"))
        .expect("[main] Unable to parse numberic xlen.");
    if xlen != 64 {
        warn!("uclidinterface is hard-coded with 64 bit dependent definitions.");
        panic!("[main] Non-64 bit XLEN is not yet implemented.");
    }
    // Parse function blocks from binary
    let binary_paths = matches
        .value_of("binaries")
        .map_or(vec![], |lst| lst.split(",").collect::<Vec<&str>>());
    // Disassemble binaries and create basic blocks
    let mut disassembler = Disassembler::new(None, Some("debug_log"));
    let als = disassembler.read_binaries(&binary_paths);
    let bbs = BasicBlock::split(&als);

    // Module name
    let module_name = matches.value_of("modname").unwrap_or("main");
    // Initialize DWARF reader
    let dwarf_reader: Rc<DwarfReader<CDwarfInterface>> =
        Rc::new(DwarfReader::new(&xlen, &binary_paths).unwrap());
    // Function to generate
    let func_names = matches
        .value_of("function")
        .map_or(vec![], |lst| lst.split(",").collect::<Vec<&str>>());
    // Specification
    let spec_files = matches
        .value_of("spec")
        .map_or(vec![], |lst| lst.split(",").collect::<Vec<&str>>());
    let specs_map = process_specs(&spec_files, &dwarf_reader.ctx());
    // Get ignored functions
    let ignored_funcs = matches
        .value_of("ignore-funcs")
        .map_or(HashSet::new(), |lst| {
            lst.split(",").collect::<HashSet<&str>>()
        });
    // Get list of functions to verify
    let verify_funcs = matches
        .value_of("verify-funcs")
        .map_or(vec![], |lst| lst.split(",").collect::<Vec<&str>>());
    // Flag for ignoring and inlining functions
    let ignore_specs = matches.is_present("ignore-specs");

    // Print all the vectre programs
    if let Some(vectre_output_file) = matches.value_of("vectre_programs") {
        let name_to_addr_map = Translator::<Uclid5Interface>::create_label_to_addr_map(&bbs);
        let programs_str = VectreProgramGenerator::get_vectre_programs_by_bb(&func_names.iter().cloned().collect::<HashSet<&str>>(), &bbs, &name_to_addr_map);
        let res = File::create(vectre_output_file)
            .ok()
            .unwrap()
            .write_all(programs_str.as_bytes());
        match res {
            Ok(_) => info!(
                "Successfully wrote vectre programs to {}",
                vectre_output_file,
            ),
            Err(_) => panic!("Unable to write vectre programs to {}", vectre_output_file),
        }
        return;
    }

    // Print all specification template
    if let Some(output_file) = matches.value_of("spec_template") {
        let funcs: HashSet<String> = dwarf_reader.ctx().func_sigs().keys().cloned().collect();
        let spec_template_str = SpecTemplateGenerator::fun_templates(&funcs, dwarf_reader.ctx());
        let res = File::create(output_file)
            .ok()
            .unwrap()
            .write_all(spec_template_str.as_bytes());
        match res {
            Ok(_) => info!(
                "Successfully wrote specification template to {}",
                output_file
            ),
            Err(_) => panic!("Unable to write specificaiton template to {}", output_file),
        }
        return;
    }

    // Translate and write to output file
    let mut translator: Translator<Uclid5Interface> = Translator::new(
        xlen,
        &module_name,
        &bbs,
        &ignored_funcs,
        &verify_funcs,
        dwarf_reader.ctx(),
        &specs_map,
        ignore_specs,
    );
    for func_name in func_names {
        translator.gen_func_model(&func_name);
    }
    // Print model to file
    let model_str = translator.print_model();
    if let Some(output_file) = matches.value_of("output") {
        let res = File::create(output_file)
            .ok()
            .unwrap()
            .write_all(model_str.as_bytes());
        match res {
            Ok(_) => info!("Successfully wrote model to {}", output_file),
            Err(_) => panic!("Unable to write model to {}", output_file),
        }
    }
    translator.clear();
    return;
}

// ===========================================================================================
/// # Command Line Interface

/// Returns options App struct for the VERI-V tool
fn cl_options<'t, 's>() -> App<'t, 's> {
    App::new("RISCVerifier")
        .version("1.0")
        .author("Kevin Cheang <kcheang@berkeley.edu>")
        .about("Translates RISC-V assembly (support for 64g only) programs into an IR")
        .arg(
            Arg::with_name("binaries")
                .short("b")
                .long("binary")
                .help("RISC-V binary file.")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("modname")
                .short("n")
                .long("modname")
                .help("UCLID module name")
                .required(false)
                .takes_value(true),
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
            Arg::with_name("spec_template")
                .help("Specify the specification template output file.")
                .short("t")
                .long("spec_template")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("vectre_programs")
                .help("Specify the vectre programs output file.")
                .long("vectre_programs")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("function")
                .help("Specify a list of functions to verify.")
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
            Arg::with_name("verify-funcs")
                .help("List of functions to verify.")
                .short("v")
                .long("verify-funcs")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("ignore-specs")
                .help("List of functions to verify.")
                .long("ignore-specs")
                .takes_value(false),
        )
}

// ====================================================================================================
/// # Specifications

/// Returns a map from function names to a vector of specifications
///
/// # Arguments
///
/// * `spec_files` - String of filenames containing specifications
///
/// * `dwarf_ctx` - DWARF context containing debugging information from the binary
pub fn process_specs(
    spec_files: &Vec<&str>,
    dwarf_ctx: &DwarfCtx,
) -> HashMap<String, Vec<sl_ast::Spec>> {
    // Parse specifications
    let spec_parser = sl_parser::SpecParser::new();
    let fun_specs_vec = spec_parser
        .process_spec_files(spec_files)
        .expect("Could not read spec.");

    // Run a set of passes over each individual specification expression
    let mut ret = HashMap::new();
    for fun_spec in fun_specs_vec {
        let fname = fun_spec.fname.to_string();
        let rw_specs = fun_spec
            .specs
            .into_iter()
            .map(|spec| match spec {
                sl_ast::Spec::Requires(bexpr) => {
                    sl_ast::Spec::Requires(sl_bexpr_rewrite_passes(bexpr, dwarf_ctx, &fname[..]))
                }
                sl_ast::Spec::Ensures(bexpr) => {
                    sl_ast::Spec::Ensures(sl_bexpr_rewrite_passes(bexpr, dwarf_ctx, &fname[..]))
                }
                _ => spec,
            })
            .collect::<Vec<_>>();
        ret.insert(fname, rw_specs);
    }
    ret
}

/// Iterates over all spec AST passes
fn sl_bexpr_rewrite_passes(
    bexpr: sl_ast::BExpr,
    dwarf_ctx: &DwarfCtx,
    fname: &str,
) -> sl_ast::BExpr {
    // Type inference pass. Before the initial pass, we expect the specficiation
    // AST to have Unknown types for all VExpr.
    let mut rw_bexpr = VExprTypeInference::visit_bexpr(
        bexpr,
        &RefCell::new((dwarf_ctx, fname, &mut HashMap::new())),
    );

    // Rewrite all quantified variable names. Identifiers that are global variables are
    // replaced with a function application and prefix that calls an alias.
    rw_bexpr = RenameGlobals::visit_bexpr(rw_bexpr, &RefCell::new(dwarf_ctx));

    // Constant folding on the expressions
    rw_bexpr = ConstantFolder::visit_bexpr(rw_bexpr, &RefCell::new(dwarf_ctx));

    // Return rewritten bexpr
    rw_bexpr
}

// ====================================================================================================
/// ## Specification transformation passes

/// AST pass that renames the identifiers for global variables from
/// Identifiers `name` to FuncApp `global_var_name()`.
struct RenameGlobals;
impl sl_ast::ASTRewriter<&DwarfCtx> for RenameGlobals {
    fn rewrite_vexpr_ident(ident: sl_ast::VExpr, context: &RefCell<&DwarfCtx>) -> sl_ast::VExpr {
        if is_global(&ident, &*context.borrow()) {
            match ident {
                sl_ast::VExpr::Ident(name, typ) => {
                    let global_addr = context.borrow().global_var(&name).expect("Could not find global variable").memory_addr;
                    sl_ast::VExpr::Bv { value: global_addr, typ }
                }
                _ => panic!("Expected identifier."),
            }
        } else {
            ident
        }
    }
}

/// AST pass that automatically infers and rewrites the type of the VExpr
/// In addition to inferring the type, it automatically injects dereferences
/// if the expression refers to a global variable and is a primitive (no dereferences
/// for non-primitives because they can be large). 
struct VExprTypeInference;
impl sl_ast::ASTRewriter<(&DwarfCtx, &str, &mut HashMap<String, sl_ast::VType>)>
    for VExprTypeInference
{
    /// Add the bound variable to the type map when it's encountered in a quantifier
    fn rewrite_bexpr_boolop(
        vop: sl_ast::BoolOp,
        context: &RefCell<(&DwarfCtx, &str, &mut HashMap<String, sl_ast::VType>)>,
    ) -> sl_ast::BoolOp {
        let mut borrowed_ctx = context.borrow_mut();
        // Add the types of the bound variables to the type map
        match &vop {
            sl_ast::BoolOp::Forall(v, _) => borrowed_ctx
                .2
                .insert(v.get_ident_name().to_string(), v.typ().clone()),
            sl_ast::BoolOp::Exists(v, _) => borrowed_ctx
                .2
                .insert(v.get_ident_name().to_string(), v.typ().clone()),
            _ => None,
        };
        vop
    }

    /// Replace the identifiers with their actual types (instead of unknown)
    fn rewrite_vexpr_ident(
        vexpr: sl_ast::VExpr,
        context: &RefCell<(&DwarfCtx, &str, &mut HashMap<String, sl_ast::VType>)>,
    ) -> sl_ast::VExpr {
        let borrowed_ctx = context.borrow();
        // Unpack the context tuple
        let dwarf_ctx = borrowed_ctx.0;
        let fname = borrowed_ctx.1;
        let typ_map = &borrowed_ctx.2;

        // Initialize a type option to store the identifier type
        let mut typ_opt;
        // Identifier name
        let var_id = vexpr.get_ident_name();

        // Check if it's a system variable
        let xlen = dwarf_ctx.xlen;
        typ_opt = match &var_id[..] {
            constants::PC_VAR => {
                Some(sl_ast::VType::from_ast_type(&system_model::pc_type(xlen)))
            }
            constants::RETURNED_FLAG => {
                Some(sl_ast::VType::from_ast_type(&system_model::returned_type()))
            }
            constants::PRIV_VAR => {
                Some(sl_ast::VType::from_ast_type(&system_model::priv_type()))
            }
            constants::MEM_VAR_B => Some(sl_ast::VType::from_ast_type(
                &system_model::mem_b_type(xlen),
            )),
            constants::MEM_VAR_H => Some(sl_ast::VType::from_ast_type(
                &system_model::mem_h_type(xlen),
            )),
            constants::MEM_VAR_W => Some(sl_ast::VType::from_ast_type(
                &system_model::mem_w_type(xlen),
            )),
            constants::MEM_VAR_D => Some(sl_ast::VType::from_ast_type(
                &system_model::mem_d_type(xlen),
            )),
            // TODO: Should include all registers, not just these
            constants::A0 | constants::SP | constants::RA => {
                Some(sl_ast::VType::from_ast_type(&system_model::bv_type(xlen)))
            }
            _ => None,
        };

        // Check if it's a formal argument
        let func_sig_opt = dwarf_ctx.func_sigs().get(&fname[..]);
        if let Some(func_sig) = func_sig_opt {
            let formal_arg_opt = func_sig.args.iter().find(|dv| dv.name == var_id);
            if let Some(fa) = formal_arg_opt {
                typ_opt = Some(from_dwarf_type(&fa.typ_defn));
            }
        }

        // Bound variable; find in type map `typ_map`
        typ_opt = typ_opt.or_else(|| {
            typ_map
                .get(&var_id[..])
                .map_or(None, |typ| Some(typ.clone()))
        });

        // Global variable
        // NOTE: formals takes shadow precedence over globals as usual
        let mut is_global = false;
        if typ_opt.is_none() {
            let dt_res = dwarf_ctx.global_var_type(&var_id);
            if dt_res.is_err() {
                // Unable to find any type. Must be struct field.
                // In this case, we don't need to return the type.
                // Maybe we should so that it's easier to refer to,
                // but the parent node should be a struct type
                // that contains the type of this field select.
                return vexpr;
            }
            typ_opt = Some(from_dwarf_type(&dt_res.unwrap()));
            is_global = true;
        }

        assert!(
            typ_opt.is_some(),
            "Unable to find variable {} in DWARF info.",
            &var_id
        );
        let global_addr = sl_ast::VExpr::Ident(format!("{}", var_id), typ_opt.clone().unwrap());
        if is_global {
            match &typ_opt.clone().unwrap() {
                sl_ast::VType::Bv(_) => {
                    // Primitive type; dereference this global
                    sl_ast::VExpr::OpApp(
                        sl_ast::ValueOp::Deref,
                        vec![global_addr],
                        typ_opt.clone().unwrap(),
                    )
                }
                _ => global_addr,
            }
        } else {
            // FIXME: formal arguments all have xlen bit width (for now)
            sl_ast::VExpr::Ident(var_id.to_string(), sl_ast::VType::Bv(dwarf_ctx.xlen as u16))
        }
    }

    /// Infer types for the operator applications of VExprs
    /// and insert dereferences where necessary
    fn rewrite_vexpr_opapp(
        opapp: sl_ast::VExpr,
        _context: &RefCell<(&DwarfCtx, &str, &mut HashMap<String, sl_ast::VType>)>,
    ) -> sl_ast::VExpr {
        match opapp {
            sl_ast::VExpr::OpApp(op, exprs, _) => {
                // infer the type of the operand after rewriting the operands
                let rw_typ = sl_ast::VType::infer_op_type(&op, &exprs);
                // insert dereferences
                match &op {
                    sl_ast::ValueOp::GetField => {
                        // Implicit dereference for field operator if the type is a primitive
                        let struct_type = &exprs[0].typ();
                        let field_name = &exprs[1].get_ident_name();
                        let field_type = match struct_type {
                            sl_ast::VType::Struct {
                                id: _,
                                fields,
                                size: _,
                            } => *fields
                                .get(&field_name[..])
                                .expect(&format!("Unable to find struct field {}.", field_name))
                                .clone(),
                            _ => panic!("Expected struct type for variable {:?}.", exprs[0]),
                        };
                        // This opapp has the field type infered
                        let field_ident = sl_ast::VExpr::Ident(
                            exprs[1].get_ident_name().to_string(),
                            field_type.clone(),
                        );
                        let select_opapp = sl_ast::VExpr::OpApp(
                            op,
                            vec![exprs[0].clone(), field_ident],
                            rw_typ,
                        );
                        match &field_type {
                            sl_ast::VType::Bv(_) => sl_ast::VExpr::OpApp(
                                sl_ast::ValueOp::Deref,
                                vec![select_opapp],
                                field_type,
                            ),
                            _ => select_opapp,
                        }
                    }
                    sl_ast::ValueOp::ArrayIndex => {
                        // Implicit dereference for array index if the type is a primitive
                        match &exprs[0].typ().clone() {
                            sl_ast::VType::Array {
                                in_type: _,
                                out_type,
                            } => match &**out_type {
                                sl_ast::VType::Bv(_) => {
                                    let rw_opapp = sl_ast::VExpr::OpApp(op, exprs, rw_typ);
                                    sl_ast::VExpr::OpApp(
                                        sl_ast::ValueOp::Deref,
                                        vec![rw_opapp],
                                        *out_type.clone(),
                                    )
                                }
                                _ => sl_ast::VExpr::OpApp(op, exprs, rw_typ),
                            },
                            _ => panic!("Expected array type for variable {:?}", exprs[0]),
                        }
                    }
                    // Update the expressions and infer the type for everything else
                    _ => sl_ast::VExpr::OpApp(op, exprs, rw_typ),
                }
            }
            _ => panic!("Implementation error; expected `VExpr::OpApp`."),
        }
    }

    fn rewrite_vexpr_funcapp(
        funcapp: sl_ast::VExpr,
        context: &RefCell<(&DwarfCtx, &str, &mut HashMap<String, sl_ast::VType>)>,
    ) -> sl_ast::VExpr {
        match funcapp {
            sl_ast::VExpr::FuncApp(fid, exprs, _) => {
                let rw_fid = Self::rewrite_vexpr_funcid(fid, context);
                let rw_exprs = exprs.into_iter().map(|expr| Self::visit_vexpr(expr, context)).collect::<Vec<_>>();
                let rw_typ = sl_ast::VType::infer_func_app_type(&rw_fid, &rw_exprs);
                sl_ast::VExpr::FuncApp(rw_fid, rw_exprs, rw_typ)
            }
            _ => panic!("Implementation error; expected `VExpr::FuncApp`."),
        }
    }
}

/// AST pass that constant folds expressions
struct ConstantFolder;
impl ConstantFolder {
    fn constant_fold(vexpr: sl_ast::VExpr, ctx: &RefCell<&DwarfCtx>) -> sl_ast::VExpr {
        match vexpr {
            sl_ast::VExpr::OpApp(value_op, operands, typ) => {
                let rw_operands = operands.into_iter().map(|operand| Self::constant_fold(operand, ctx)).collect::<Vec<_>>();
                let oper1 = rw_operands.get(0).unwrap();
                let oper2_opt = rw_operands.get(1);    // second operand only appears in some operations
                if !(oper1.is_lit() && oper2_opt.map_or(true, |oper| oper.is_lit())) {
                    return sl_ast::VExpr::OpApp(value_op, rw_operands, typ);
                }
                let oper1_val: u64 = oper1.get_lit_value().expect("Expected at least one operand.");
                let oper2_val_opt: Option<u64> = oper2_opt.map(|oper| oper.get_lit_value().unwrap());
                match value_op {
                    sl_ast::ValueOp::Add => sl_ast::VExpr::Bv { value: oper1_val + oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::Sub => sl_ast::VExpr::Bv { value: oper1_val - oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::Div => sl_ast::VExpr::Bv { value: oper1_val / oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::Mul => sl_ast::VExpr::Bv { value: oper1_val * oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::BvXor => sl_ast::VExpr::Bv { value: oper1_val ^ oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::BvOr => sl_ast::VExpr::Bv { value: oper1_val | oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::BvAnd => sl_ast::VExpr::Bv { value: oper1_val & oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::RightShift => sl_ast::VExpr::Bv { value: ((oper1_val as i64) >> oper2_val_opt.unwrap()) as u64, typ: oper1.typ().clone() },
                    sl_ast::ValueOp::URightShift => sl_ast::VExpr::Bv { value: oper1_val >> oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::LeftShift => sl_ast::VExpr::Bv { value: oper1_val << oper2_val_opt.unwrap(), typ: oper1.typ().clone() },
                    sl_ast::ValueOp::Slice { lo, hi } => {
                        let typ = sl_ast::VType::Bv(oper1.typ().get_bv_width() + oper2_opt.unwrap().typ().get_bv_width());
                        sl_ast::VExpr::Bv { value: oper1_val & helpers::mask(hi as u64, lo as u64), typ }
                    }
                    sl_ast::ValueOp::ArrayIndex => {
                        let out_typ_bytes = oper1.typ().get_array_out_type_size() / constants::BYTE_SIZE;
                        let index = oper2_val_opt.expect("Expected two arguments for array index.");
                        let base_addr = oper1.get_lit_value().expect("Array should be bv lit now from rename globals pass.");
                        sl_ast::VExpr::Bv { value: base_addr + out_typ_bytes*index, typ: oper1.typ().get_array_out_type().clone() }
                    },
                    sl_ast::ValueOp::Deref => {
                        if oper1.is_lit() {
                            sl_ast::VExpr::Ident(helpers::abs_access_name(&oper1.get_lit_value().unwrap()), typ)
                        } else {
                            sl_ast::VExpr::OpApp(value_op, rw_operands, typ)
                        }
                    },
                    // TODO: Implement remaining
                    sl_ast::ValueOp::Concat => sl_ast::VExpr::OpApp(value_op, rw_operands, typ),
                    sl_ast::ValueOp::GetField => sl_ast::VExpr::OpApp(value_op, rw_operands, typ),
                }
            }
            _ => vexpr
        }
    }
}

impl sl_ast::ASTRewriter<&DwarfCtx> for ConstantFolder {
    fn rewrite_vexpr(opapp: sl_ast::VExpr, ctx: &RefCell<&DwarfCtx>) -> sl_ast::VExpr {
        Self::constant_fold(opapp, ctx)
    }
}

// ================================================================================
/// # DWARF Helpers

/// Translates a DwarfTypeDefn to a specification variable type
fn from_dwarf_type(dtd: &DwarfTypeDefn) -> sl_ast::VType {
    match dtd {
        DwarfTypeDefn::Primitive { bytes }
        | DwarfTypeDefn::Pointer {
            value_typ: _,
            bytes,
        } => sl_ast::VType::Bv((bytes * constants::BYTE_SIZE) as u16),
        DwarfTypeDefn::Array {
            in_typ,
            out_typ,
            bytes: _,
        } => sl_ast::VType::Array {
            in_type: Box::new(from_dwarf_type(in_typ)),
            out_type: Box::new(from_dwarf_type(out_typ)),
        },
        DwarfTypeDefn::Struct { id, fields, bytes } => {
            let id = id.to_string();
            let fields = fields
                .iter()
                .map(|kv| {
                    let field_name = (&*kv.0).clone();
                    let field_type = from_dwarf_type(&*kv.1.typ);
                    (field_name, Box::new(field_type))
                })
                .collect::<HashMap<String, Box<sl_ast::VType>>>();
            let size = bytes * constants::BYTE_SIZE;
            sl_ast::VType::Struct { id, fields, size }
        }
    }
}

/// Helper function that determines if one of the VExprs from `vexprs`
/// is a global variable
pub fn is_global(vexpr: &sl_ast::VExpr, dwarf_ctx: &DwarfCtx) -> bool {
    match vexpr {
        sl_ast::VExpr::Ident(name, _) => dwarf_ctx.global_var(&name[..]).is_ok(),
        sl_ast::VExpr::OpApp(_, vexprs, _) | sl_ast::VExpr::FuncApp(_, vexprs, _) => {
            has_global(vexprs, dwarf_ctx)
        }
        _ => false,
    }
}

/// Helper function that determines if one of the sl_ast::VExprs from `vexprs`
/// is a global variable
pub fn has_global(vexprs: &Vec<sl_ast::VExpr>, dwarf_ctx: &DwarfCtx) -> bool {
    vexprs
        .iter()
        .fold(false, |acc, vexpr| acc || is_global(vexpr, dwarf_ctx))
}
