use std::collections::HashSet;
use std::fs;
use std::rc::Rc;

use crate::ast::*;
use crate::ir_interface::{IRInterface, SpecLangASTInterface};
use crate::readers::dwarfreader::{DwarfCtx, DwarfTypeDefn, DwarfVar};
use crate::spec_lang::sl_ast;
// use crate::system_model;
use crate::utils;

#[derive(Debug)]
pub struct Uclid5Interface;

impl Uclid5Interface {
    /// Returns a string of the variable declarations in the model
    ///
    /// # Arguments
    ///
    /// * `model` - The model to generate the declarations string for
    fn gen_var_defns(model: &Model) -> String {
        let mut sorted = model.vars.iter().collect::<Vec<_>>();
        sorted.sort();
        let defns = sorted
            .iter()
            .map(|v| format!("var {};", Self::var_decl(v)))
            .collect::<Vec<String>>()
            .join("\n");
        format!("// RISC-V system state variables\n{}", defns)
    }
    /// Reads the model for the RISC-V instructions (provided by utils::PRELUDE_PATH) and returns it as a string
    fn prelude() -> String {
        fs::read_to_string(utils::PRELUDE_PATH).expect("Unable to read prelude.")
    }
    /// Generate a define macro string for each type of array variable
    /// that is a global variable or function argument
    ///
    /// # Arguments
    ///
    /// * `dwarf_ctx` - The DWARF information that contains all the global variables and function
    ///                 signatures for the binaries provided
    fn gen_array_defns(dwarf_ctx: &DwarfCtx, xlen: &u64) -> String {
        let mut defns: Vec<String> = vec![];
        for var in dwarf_ctx.global_vars() {
            defns.append(&mut Self::gen_array_defn(&var.typ_defn, xlen));
        }
        for (_, func_sig) in dwarf_ctx.func_sigs() {
            for var in &func_sig.args {
                defns.append(&mut Self::gen_array_defn(&var.typ_defn, xlen));
            }
            if let Some(ret_type) = &func_sig.ret_type {
                defns.append(&mut Self::gen_array_defn(&ret_type, xlen));
            }
        }
        defns.sort();
        defns.dedup();
        utils::indent_text(format!("// Array helpers\n{}", defns.join("\n")), 4)
    }
    /// Recursively generate define macros for a given type (size in bytes).
    /// The macro is a function that takes a base address and index
    /// and returns 'base + index * typ_defn.bytes'.
    /// Returns a string representations of these define macros.
    ///
    /// # Arguments
    ///
    /// * `typ_defn` - Type to generate an array index macro for
    ///
    /// # Example
    ///
    /// define index_by_16(base: xlen_t, index: xlen_t): xlen_t = base + bv_left_shift(to_xlen_t(4bv64), index);
    fn gen_array_defn(typ_defn: &DwarfTypeDefn, xlen: &u64) -> Vec<String> {
        let mut defns = vec![];
        match &typ_defn {
            DwarfTypeDefn::Primitive { bytes } => {
                // Check if the type is valid (bytes > 0)
                if *bytes > 0 {
                    defns.push(format!(
                        "define {}(base: bv{}, index: bv{}): bv{} = base + {};",
                        Self::array_index_macro_name(bytes),
                        xlen,
                        xlen,
                        xlen,
                        if *bytes == 1 {
                            format!("index")
                        } else {
                            Self::multiply_expr(bytes, "index", xlen)
                        }
                    ))
                }
            }
            DwarfTypeDefn::Array {
                in_typ,
                out_typ,
                bytes: _,
            } => {
                defns.append(&mut Self::gen_array_defn(in_typ, xlen));
                defns.append(&mut Self::gen_array_defn(out_typ, xlen));
            }
            DwarfTypeDefn::Struct {
                id: _,
                fields,
                bytes,
            } => {
                for (_, field) in fields {
                    defns.append(&mut Self::gen_array_defn(&field.typ, xlen));
                }
                if *bytes > 0 {
                    defns.push(format!(
                        "define {}(base: bv{}, index: bv{}): bv{} = base + {};",
                        Self::array_index_macro_name(bytes),
                        xlen,
                        xlen,
                        xlen,
                        Self::multiply_expr(bytes, "index", xlen)
                    ))
                }
            }
            DwarfTypeDefn::Pointer {
                value_typ,
                bytes: _,
            } => defns.append(&mut Self::gen_array_defn(&value_typ, xlen)),
        };
        defns
    }
    /// Returns the name of the array index macro given the byte size
    fn array_index_macro_name(bytes: &u64) -> String {
        format!("index_by_{}", bytes)
    }
    /// Creates an expression that represents 'num_const * expr'
    /// TODO: Does SMT support precise multiplication? Maybe we can take this out
    fn multiply_expr(num_const: &u64, expr: &str, xlen: &u64) -> String {
        format!("{:b}", num_const) // Binary expression
            .chars()
            .rev()
            .fold((String::from(""), 0), |acc, x| {
                // acc = (expression, i-th bit counter)
                if x == '1' {
                    (
                        // Add multiple of 2 of the expression to the current expression
                        format!(
                            "bv_left_shift({}, {}){}{}",
                            format!("{}bv{}", acc.1, xlen),
                            expr,
                            if acc.0.len() == 0 { "" } else { " + " },
                            acc.0
                        ),
                        // Increment the i-th bit counter
                        acc.1 + 1,
                    )
                } else {
                    (acc.0, acc.1 + 1)
                }
            })
            .0
        // SLOWER:
        // format!("{} * {}bv{}", expr, num_const, xlen)
    }
    /// Return a string of get field macros for all the type definitions in the global variables
    /// and formal arguments of functions.
    ///  
    /// # Arguments
    ///
    /// * `dwarf_ctx` - The DWARF context containing the variables and function signatures.
    fn gen_struct_defns(dwarf_ctx: &DwarfCtx, xlen: &u64) -> String {
        let mut defns = vec![];
        for var in dwarf_ctx.global_vars() {
            defns.append(&mut Self::gen_struct_defn(&var.typ_defn, xlen));
        }
        for (_, func_sig) in dwarf_ctx.func_sigs() {
            for var in &func_sig.args {
                defns.append(&mut Self::gen_struct_defn(&var.typ_defn, xlen));
            }
            if let Some(ret_type) = &func_sig.ret_type {
                defns.append(&mut Self::gen_struct_defn(&ret_type, xlen));
            }
        }
        defns.sort();
        defns.dedup();
        utils::indent_text(format!("// Struct helpers\n{}", defns.join("\n")), 4)
    }
    /// Recursively generate string representations of get field macros for type definition 'typ'.
    ///
    /// # Example
    ///
    ///     Given the following struct definition:
    ///     struct ctx { ..., a0: T, ... };
    ///
    ///     This function returns the following definition to simplify 'c.a0', where c is of type ctx:
    ///     define ctx_a0(ptr: xlen_t): xlen_t = ptr + to_xlen_t(80bv64);
    fn gen_struct_defn(typ: &DwarfTypeDefn, xlen: &u64) -> Vec<String> {
        let mut defns = vec![];
        match typ {
            DwarfTypeDefn::Struct {
                id,
                fields,
                bytes: _,
            } => {
                for (field_name, field) in fields {
                    defns.append(&mut Self::gen_struct_defn(&*field.typ, xlen));
                    defns.push(format!(
                        "define {}(ptr: bv{}): bv{} = ptr + {}bv{};",
                        Self::get_field_macro_name(&id[..], field_name),
                        xlen,
                        xlen,
                        field.loc,
                        xlen
                    ));
                }
            }
            DwarfTypeDefn::Array {
                in_typ,
                out_typ,
                bytes: _,
            } => {
                defns.append(&mut Self::gen_struct_defn(&in_typ, xlen));
                defns.append(&mut Self::gen_struct_defn(&out_typ, xlen));
            }
            _ => (),
        }
        defns
    }
    /// Given the `struct_id` and `field_name`, return the get field macro name.
    fn get_field_macro_name(struct_id: &str, field_name: &String) -> String {
        format!("{}_{}", struct_id, field_name)
    }
    /// Given the dwarf_ctx, returns a string of global variable definitions.
    fn gen_global_defns(dwarf_ctx: &DwarfCtx, xlen: &u64) -> String {
        let mut defns = String::from("// Global variables\n");
        for var in dwarf_ctx.global_vars() {
            defns = format!("{}{}\n", defns, Self::gen_global_defn(&var, xlen));
        }
        utils::indent_text(defns, 4)
    }
    /// Given a global variable, returns a string of a macro that refers to the static
    /// memory location of the variable.
    fn gen_global_defn(global_var: &DwarfVar, xlen: &u64) -> String {
        format!(
            "define {}(): bv{} = {};",
            utils::global_var_ptr_name(&global_var.name[..]),
            xlen,
            format!("{}bv{}", global_var.memory_addr, xlen)
        )
    }
    /// Returns a string of macros to refer to a static function's entry address.
    fn gen_global_func_defns(model: &Model, xlen: &u64) -> String {
        let mut defns = String::from("// Global function entry addresses\n");
        for fm in &model.func_models {
            defns = format!(
                "{}{}\n",
                defns,
                Self::gen_global_func_defn(&fm.sig.name, fm.sig.entry_addr, xlen)
            );
        }
        utils::indent_text(defns, 4)
    }
    /// Returns a define macro that returns the `func_entry_addr`
    fn gen_global_func_defn(func_name: &str, func_entry_addr: u64, xlen: &u64) -> String {
        format!(
            "define {}(): bv{} = {};",
            utils::global_func_addr_name(func_name),
            xlen,
            format!("{}bv{}", func_entry_addr, xlen)
        )
    }

    fn specs_to_string(fsig: &FuncSig, dwarf_ctx: &DwarfCtx, xlen: &u64) -> String {
        let mut specs = "".to_string();
        // requires
        for require in &fsig.requires {
            // FIXME: implement this inside SpecLangInterface
            let bexpr = require.get_bexpr().unwrap();
            let require_str = Self::bexpr_to_string(bexpr);
            specs = format!("{}requires {};\n", specs, require_str);
        }
        // ensures
        for ensure in &fsig.ensures {
            let bexpr = ensure.get_bexpr().unwrap();
            let ensure_str = Self::bexpr_to_string(bexpr);
            specs = format!("{}ensures {};\n", specs, ensure_str);
        }
        specs
    }

    /// Returns a string of all the procedures in the model.
    /// This contains all of the function models.
    fn gen_procs(model: &Model, dwarf_ctx: &DwarfCtx, xlen: &u64) -> String {
        let procs_string = model
            .func_models
            .iter()
            .map(|fm| {
                // FIXME: pass into func_model
                let specs = Self::specs_to_string(&fm.sig, dwarf_ctx, xlen);
                println!("SPECS: {}", specs);
                Self::func_model_to_string(fm, dwarf_ctx, xlen)
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        utils::indent_text(procs_string, 4)
    }
    /// Returns the control block for the UCLID5 model.
    /// This currently will automatically verify all functions with
    /// a specification.
    fn control_blk(
        model: &Model,
        dwarf_ctx: &DwarfCtx,
        ignored_funcs: &HashSet<&str>,
        verify_funcs: &Vec<&str>,
    ) -> String {
        let verif_fns_string = if verify_funcs.len() > 0 {
            verify_funcs
                .iter()
                .map(|f_name| format!("f{} = verify({});", f_name, f_name))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            model
                .func_models
                .iter()
                .filter(|fm| dwarf_ctx.func_sig(&fm.sig.name).is_ok())
                .map(|fm| {
                    if !ignored_funcs.contains(&fm.sig.name[..]) {
                        format!(
                            "f{} = verify({});",
                            fm.sig.name.clone(),
                            fm.sig.name.clone()
                        )
                    } else {
                        String::from("")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let verif_fns_string = format!("{}\ncheck;\nprint_results;", verif_fns_string);
        let verif_fns_string = utils::indent_text(verif_fns_string, 4);
        let solver_opts = utils::indent_text(format!("set_solver_option(\":mbqi\", false);\nset_solver_option(\":case_split\", 0);\nset_solver_option(\":relevancy\", 0);\nset_solver_option(\":blast_full\", true);"), 4);
        let control_string = format!("control {{\n{}\n{}\n}}", solver_opts, verif_fns_string);
        utils::indent_text(control_string, 4)
    }

    /// =================== Helper functions ===================

    /// Return a UCLID5 variable declaration.
    ///
    /// # Example
    ///
    /// Var = { name: "x".to_string(), typ: Type::Bv { bytes: 64 } } will return:
    /// `x: bv64`
    fn var_decl(var: &Var) -> String {
        format!(
            "{}: {}",
            Self::var_to_string(var),
            Self::typ_to_string(&var.typ)
        )
    }
}

impl IRInterface for Uclid5Interface {
    /// IR translation functions
    fn lit_to_string(lit: &Literal) -> String {
        match lit {
            Literal::Bv { val, width } => format!("{}bv{}", *val as i64, width),
            Literal::Bool { val } => format!("{}", val),
            Literal::Int { val } => format!("{}", val),
        }
    }
    fn typ_to_string(typ: &Type) -> String {
        match typ {
            Type::Unknown => panic!("Type is unknown!"),
            Type::Bool => format!("boolean"),
            Type::Int => format!("integer"),
            Type::Bv { w } => format!("bv{}", w),
            Type::Array { in_typs, out_typ } => format!(
                "[{}]{}",
                in_typs
                    .iter()
                    .map(|typ| Self::typ_to_string(typ))
                    .collect::<Vec<_>>()
                    .join(", "),
                Self::typ_to_string(out_typ)
            ),
            Type::Struct {
                id: _,
                fields: _,
                w: _,
            } => panic!("Should not need to print struct types in this model."),
        }
    }
    fn comp_app_to_string(compop: &CompOp, e1: Option<String>, e2: Option<String>) -> String {
        match compop {
            CompOp::Equality => format!("({} == {})", e1.unwrap(), e2.unwrap()),
            CompOp::Inequality => format!("({} != {})", e1.unwrap(), e2.unwrap()),
            CompOp::Lt => format!("({} < {})", e1.unwrap(), e2.unwrap()),
            CompOp::Le => format!("({} <= {})", e1.unwrap(), e2.unwrap()),
            CompOp::Gt => format!("({} > {})", e1.unwrap(), e2.unwrap()),
            CompOp::Ge => format!("({} >= {})", e1.unwrap(), e2.unwrap()),
            CompOp::Ltu => format!("({} <_u {})", e1.unwrap(), e2.unwrap()),
            CompOp::Leu => format!("({} <=_u {})", e1.unwrap(), e2.unwrap()),
            CompOp::Gtu => format!("({} >_u {})", e1.unwrap(), e2.unwrap()),
            CompOp::Geu => format!("({} >=_u {})", e1.unwrap(), e2.unwrap()),
        }
    }
    fn bv_app_to_string(bvop: &BVOp, e1: Option<String>, e2: Option<String>) -> String {
        match bvop {
            BVOp::Add => format!("({} + {})", e1.unwrap(), e2.unwrap()),
            BVOp::Sub => format!("({} - {})", e1.unwrap(), e2.unwrap()),
            BVOp::Mul => format!("({} * {})", e1.unwrap(), e2.unwrap()),
            BVOp::And => format!("({} & {})", e1.unwrap(), e2.unwrap()),
            BVOp::Or => format!("({} | {})", e1.unwrap(), e2.unwrap()),
            BVOp::Xor => format!("({} ^ {})", e1.unwrap(), e2.unwrap()),
            BVOp::SignExt => match e2.unwrap().split("bv").next().unwrap() {
                width if width != "0" => format!("bv_sign_extend({}, {})", width, e1.unwrap()),
                _ => format!("{}", e1.unwrap()),
            },
            BVOp::ZeroExt => match e2.unwrap().split("bv").next().unwrap() {
                width if width != "0" => format!("bv_zero_extend({}, {})", width, e1.unwrap()),
                _ => format!("{}", e1.unwrap()),
            },
            BVOp::LeftShift => format!("bv_left_shift({}, {})", e2.unwrap(), e1.unwrap()),
            BVOp::RightShift => format!("bv_l_right_shift({}, {})", e2.unwrap(), e1.unwrap()),
            BVOp::ARightShift => format!("bv_a_right_shift({}, {})", e2.unwrap(), e1.unwrap()),
            BVOp::Concat => format!("({} ++ {})", e1.unwrap(), e2.unwrap()),
            BVOp::Slice { l, r } => format!("{}[{}:{}]", e1.unwrap(), l, r),
        }
    }
    fn bool_app_to_string(bop: &BoolOp, e1: Option<String>, e2: Option<String>) -> String {
        match bop {
            BoolOp::Conj => format!("({} && {})", e1.unwrap(), e2.unwrap()),
            BoolOp::Disj => format!("({} || {})", e1.unwrap(), e2.unwrap()),
            BoolOp::Iff => format!("({} <==> {})", e1.unwrap(), e2.unwrap()),
            BoolOp::Impl => format!("({} ==> {})", e1.unwrap(), e2.unwrap()),
            BoolOp::Neg => format!("!{}", e1.unwrap()),
        }
    }
    fn fapp_to_string(fapp: &FuncApp, xlen: &u64) -> String {
        format!(
            "{}({})",
            fapp.func_name,
            fapp.operands
                .iter()
                .map(|x| { Self::expr_to_string(&*x, xlen) })
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
    fn var_to_string(var: &Var) -> String {
        format!("{}", var.name)
    }
    fn array_index_to_string(e1: String, e2: String) -> String {
        format!("{}[{}]", e1, e2)
    }
    fn get_field_to_string(e1: String, field: String) -> String {
        format!("{}.{}", e1, field)
    }

    /// Statements to string
    fn stmt_to_string(stmt: &Stmt, xlen: &u64) -> String {
        match stmt {
            Stmt::Assume(expr) => Self::assume_to_string(&expr, xlen),
            Stmt::FuncCall(fc) => Self::func_call_to_string(&fc, xlen),
            Stmt::Assign(assign) => Self::assign_to_string(&assign, xlen),
            Stmt::IfThenElse(ite) => Self::ite_to_string(&ite, xlen),
            Stmt::Block(stmt_vec) => Self::block_to_string(&stmt_vec, xlen),
            Stmt::Comment(comment) => Self::comment_to_string(&comment),
        }
    }
    fn skip_to_string() -> String {
        format!("")
    }
    fn assert_to_string(expr: &Expr, xlen: &u64) -> String {
        format!("assert ({});", Self::expr_to_string(expr, xlen))
    }
    fn assume_to_string(expr: &Expr, xlen: &u64) -> String {
        format!("assume ({});", Self::expr_to_string(expr, xlen))
    }
    fn havoc_to_string(var: &Rc<Var>) -> String {
        format!("havoc {};", Self::var_to_string(&*var))
    }
    fn func_call_to_string(func_call: &FuncCall, xlen: &u64) -> String {
        let lhs = func_call
            .lhs
            .iter()
            .map(|rc_expr| Self::expr_to_string(&*rc_expr, xlen))
            .collect::<Vec<_>>()
            .join(", ");
        let args = func_call
            .operands
            .iter()
            .map(|rc_expr| {
                let expr_str = Self::expr_to_string(&*rc_expr, xlen);
                if expr_str == "zero" {
                    format!("0bv{}", xlen)
                } else {
                    expr_str
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "call ({}) = {}({});",
            lhs,
            func_call.func_name.replace(".", "_"),
            args
        )
    }
    fn assign_to_string(assign: &Assign, xlen: &u64) -> String {
        let lhs = assign
            .lhs
            .iter()
            .map(|rc_expr| Self::expr_to_string(&*rc_expr, xlen))
            .collect::<Vec<_>>()
            .join(", ");
        let rhs = assign
            .rhs
            .iter()
            .map(|rc_expr| Self::expr_to_string(&*rc_expr, xlen))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} = {};", lhs, rhs)
    }
    fn ite_to_string(ite: &IfThenElse, xlen: &u64) -> String {
        let cond = Self::expr_to_string(&ite.cond, xlen);
        let thn = utils::indent_text(Self::stmt_to_string(&*ite.then_stmt, xlen), 4);
        let els = if let Some(else_stmt) = &ite.else_stmt {
            format!(
                "else {{\n{}\n}}",
                utils::indent_text(Self::stmt_to_string(&*else_stmt, xlen), 4)
            )
        } else {
            String::from("")
        };
        format!("if ({}) {{\n{}\n}}{}", cond, thn, els)
    }
    fn block_to_string(blk: &Vec<Box<Stmt>>, xlen: &u64) -> String {
        let inner = blk
            .iter()
            .map(|rc_stmt| Self::stmt_to_string(rc_stmt, xlen))
            .collect::<Vec<_>>()
            .join("\n");
        let inner = utils::indent_text(inner, 4);
        format!("{{\n{}\n}}", inner)
    }
    fn comment_to_string(string: &String) -> String {
        format!("// {}\n", string)
    }
    fn func_model_to_string(fm: &FuncModel, dwarf_ctx: &DwarfCtx, xlen: &u64) -> String {
        let args = fm
            .sig
            .arg_decls
            .iter()
            .map(|var| Self::var_decl(&var.get_expect_var()))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = if let Some(rd) = &fm.sig.ret_decl {
            format!(" returns (ret: {})", Self::typ_to_string(rd))
        } else {
            format!("")
        };
        let modifies = if fm.sig.mod_set.len() > 0 {
            format!(
                "\n    modifies {};",
                fm.sig
                    .mod_set
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            format!("")
        };
        let requires = "";
        let ensures = "";
        let body = Self::block_to_string(fm.body.get_expect_block(), xlen);
        let inline = if fm.inline { "[inline] " } else { "" };
        // Track variable procedure
        // let vt_proc = if fm.sig.tracked.len() > 0 {
        //     Self::track_proc(fm, dwarf_ctx)
        // } else {
        //     String::from("")
        // };
        let vt_proc = "";
        format!(
            "procedure {}{}({}){}{}{}{}\n{}\n\n{}",
            inline, fm.sig.name, args, ret, modifies, requires, ensures, body, vt_proc
        )
    }

    // Generate function model
    // NOTE: Replace string with write to file
    fn model_to_string(
        xlen: &u64,
        model: &Model,
        dwarf_ctx: &DwarfCtx,
        ignored_funcs: &HashSet<&str>,
        verify_funcs: &Vec<&str>,
    ) -> String {
        // prelude
        let prelude = Self::prelude();
        // variables
        let var_defns = utils::indent_text(Self::gen_var_defns(model), 4);
        // definitions
        let array_defns = Self::gen_array_defns(&dwarf_ctx, xlen); // Define macros that index for arrays (by muiltiplication)
        let struct_defns = Self::gen_struct_defns(&dwarf_ctx, xlen); // Define macros for getting struct field values
        let global_var_defns = Self::gen_global_defns(&dwarf_ctx, xlen); // Define macros for global variable pointers
        let global_func_defns = Self::gen_global_func_defns(&model, xlen); // Define macros for function addresses                                              // procedures
        let procs = Self::gen_procs(model, &dwarf_ctx, xlen);
        // control block
        let ctrl_blk = Self::control_blk(model, &dwarf_ctx, ignored_funcs, verify_funcs);
        format!(
            "module {} {{\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n\n{}\n}}",
            model.name,
            prelude,
            var_defns,
            array_defns,
            struct_defns,
            global_var_defns,
            global_func_defns,
            procs,
            ctrl_blk
        )
    }
}

impl SpecLangASTInterface for Uclid5Interface {
    /// BExpr translation functions
    fn bexpr_bool_to_string(b: &bool) -> String {
        match b {
            true => "true".to_string(),
            false => "false".to_string(),
        }
    }
    fn bexpr_bopapp_to_string(bop: &sl_ast::BoolOp, exprs: &Vec<sl_ast::BExpr>) -> String {
        let bop_str = Self::bopp_to_string(bop);
        let mut exprs_iter = exprs.iter();
        let mut ret = Self::bexpr_to_string(exprs_iter.next().unwrap());
        // Unary prefix operator
        match bop {
            sl_ast::BoolOp::Neg => return format!("{}{}", bop_str, ret),
            _ => (),
        }
        // Infix operator, comma separated by operands
        while let Some(expr) = exprs_iter.next() {
            let expr_str = Self::bexpr_to_string(expr);
            ret = format!("{} {} {}", ret, bop_str, expr_str)
        }
        ret
    }
    fn bexpr_copapp_to_string(cop: &sl_ast::CompOp, exprs: &Vec<sl_ast::VExpr>) -> String {
        assert!(
            exprs.len() == 2,
            "Invalid number of operands for comparison."
        );
        let cop_str = Self::cop_to_string(cop);
        let expr_str1 = Self::vexpr_to_string(&exprs[0]);
        let expr_str2 = Self::vexpr_to_string(&exprs[1]);
        format!("{} {} {}", expr_str1, cop_str, expr_str2)
    }
    fn bopp_to_string(bop: &sl_ast::BoolOp) -> String {
        match bop {
            sl_ast::BoolOp::Conj => "&&".to_string(),
            sl_ast::BoolOp::Disj => "||".to_string(),
            sl_ast::BoolOp::Neg => "!".to_string(),
            sl_ast::BoolOp::Implies => "==>".to_string(),
        }
    }
    fn cop_to_string(cop: &sl_ast::CompOp) -> String {
        match cop {
            sl_ast::CompOp::Equal => "==".to_string(),
            sl_ast::CompOp::Nequal => "!=".to_string(),
            sl_ast::CompOp::Gt => ">".to_string(),
            sl_ast::CompOp::Lt => "<".to_string(),
            sl_ast::CompOp::Gtu => ">_u".to_string(),
            sl_ast::CompOp::Ltu => "<_u".to_string(),
            sl_ast::CompOp::Geq => ">=".to_string(),
            sl_ast::CompOp::Leq => "<=".to_string(),
            sl_ast::CompOp::Geu => ">=_u".to_string(),
            sl_ast::CompOp::Leu => "<=_u".to_string(),
        }
    }
    // VExpr translation functions
    fn vexpr_bv_to_string(value: &u64, typ: &sl_ast::VType) -> String {
        match typ {
            sl_ast::VType::Bv(width) => format!("{}bv{}", value, width),
            _ => panic!("Should be bv typed."),
        }
    }
    fn vexpr_int_to_string(i: &i64) -> String {
        format!("{}", i)
    }
    fn vexpr_bool_to_string(b: &bool) -> String {
        match b {
            true => "true".to_string(),
            false => "false".to_string(),
        }
    }
    fn vexpr_ident_to_string(v: &String) -> String {
        v.clone()
    }
    fn vexpr_opapp_to_string(op: &sl_ast::ValueOp, exprs: &Vec<sl_ast::VExpr>) -> String {
        let op_str = Self::valueop_to_string(op);
        match op {
            sl_ast::ValueOp::Add | sl_ast::ValueOp::Sub |
            sl_ast::ValueOp::Div | sl_ast::ValueOp::Mul => {
                exprs.iter()
                    .fold(String::from(""), |acc, expr| {
                    format!("{} {} {}", acc, op_str, Self::vexpr_to_string(expr))
                })
            },
            sl_ast::ValueOp::ArrayIndex => {
                let arr = Self::vexpr_to_string(&exprs[0]);
                let index = Self::vexpr_to_string(&exprs[1]);
                let bytes = match &exprs[0].typ() {
                    sl_ast::VType::Array { in_type, out_type } => {
                        match &**out_type {
                            sl_ast::VType::Bv(w) => *w as u64 / utils::BYTE_SIZE,
                            sl_ast::VType::Struct{id:_, fields:_, size} => *size / utils::BYTE_SIZE,
                            _ => panic!("Expected BV type (op: {:#?}, exprs: {:#?}).", op, exprs),
                        }
                    },
                    _ => panic!("Expected array type."),
                };
                format!("{}({}, {}))", Self::array_index_macro_name(&bytes), arr, index)
            },
            sl_ast::ValueOp::GetField => {
                let struct_name = match &exprs[0].typ() {
                    sl_ast::VType::Struct{id, fields:_, size:_} => id,
                    _ => panic!("Expected struct type."),
                };
                let field_name = Self::vexpr_to_string(&exprs[1]);
                let expr_str = Self::vexpr_to_string(&exprs[0]);
                format!("struct_{}_{}({})", struct_name, field_name, expr_str)
            }
            _ => panic!("vexpr_to_string not implemented for {:#?}", op),
        }
    }
    fn vexpr_funcapp_to_string(fname: &String, args: &Vec<sl_ast::VExpr>) -> String {
        "v".to_string()
    }
    fn valueop_to_string(op: &sl_ast::ValueOp) -> String {
        "m".to_string()
    }
    // Spec statement to string
    fn spec_to_string(spec: &sl_ast::Spec) -> String {
        "s".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type U5I = Uclid5Interface<CDwarfInterface>;
    #[test]
    fn test_lit_to_string() {
        let bv_lit = Literal::Bv { val: 0, width: 1 };
        assert_eq!(U5I::lit_to_string(&bv_lit), "0bv1");
    }

    #[test]
    fn test_assign_to_string() {
        let bv64_type = Type::Bv { w: 64 };
        let var_x = Expr::Var(Var {
            name: "x".to_string(),
            typ: bv64_type,
        });
        let bv_lit = Expr::Literal(Literal::Bv { val: 0, width: 64 });
        let assign = Assign {
            lhs: vec![var_x],
            rhs: vec![bv_lit],
        };
        assert_eq!(U5I::assign_to_string(&assign), "x = 0bv64;");
    }
}
