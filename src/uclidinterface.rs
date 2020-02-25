use std::fs;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::dwarfreader::{DwarfInterface, DwarfReader, DwarfTypeDefn, DwarfVar};
use crate::ir::*;
use crate::translator;
use crate::utils::*;

#[derive(Debug)]
pub struct Uclid5Interface<D> {
    _phantom_d: PhantomData<D>,
}

impl<D> Uclid5Interface<D>
where
    D: DwarfInterface,
{
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
    fn prelude() -> String {
        // TODO: Put string in some config file
        fs::read_to_string("models/prelude.ucl").expect("Unable to read prelude.")
    }
    fn gen_array_defns(dwarf_reader: &Rc<DwarfReader<D>>) -> String {
        let mut defns: Vec<String> = vec![];
        for var in dwarf_reader.global_vars() {
            defns.append(&mut Self::gen_array_defn(&var.typ_defn));
        }
        for (_, func_sig) in dwarf_reader.func_sigs() {
            for var in &func_sig.args {
                defns.append(&mut Self::gen_array_defn(&var.typ_defn));
            }
            if let Some(ret_typ) = &func_sig.ret_typ_defn {
                defns.append(&mut Self::gen_array_defn(&ret_typ));
            }
        }
        defns.sort();
        defns.dedup();
        indent_text(format!("// Array helpers\n{}", defns.join("\n")), 4)
    }
    fn gen_array_defn(typ_defn: &DwarfTypeDefn) -> Vec<String> {
        let mut defns = vec![];
        match &typ_defn {
            DwarfTypeDefn::Primitive { bytes } => defns.push(format!(
                "define {}(base: xlen_t, index: xlen_t): xlen_t = base + {};",
                Self::array_index_macro_name(bytes),
                Self::multiply_expr(bytes, "index")
            )),
            DwarfTypeDefn::Array { in_typ, out_typ } => {
                defns.append(&mut Self::gen_array_defn(in_typ));
                defns.append(&mut Self::gen_array_defn(out_typ));
            }
            DwarfTypeDefn::Struct {
                id: _,
                fields,
                bytes,
            } => {
                let mut defns = vec![];
                for (_, field) in fields {
                    defns.append(&mut Self::gen_array_defn(&field.typ));
                }
                defns.push(format!(
                    "define {}(index: xlen_t): xlen_t = {};",
                    Self::array_index_macro_name(bytes),
                    Self::multiply_expr(bytes, "index")
                ))
            }
            DwarfTypeDefn::Pointer { value_typ } => {
                defns.append(&mut Self::gen_array_defn(&value_typ))
            }
        };
        defns
    }
    fn array_index_macro_name(bytes: &u64) -> String {
        format!("index_by_{}", bytes)
    }
    fn multiply_expr(num_const: &u64, expr: &str) -> String {
        format!("{:b}", num_const) // Binary expression
            .chars()
            .rev()
            .fold((String::from(""), 0), |acc, x| {
                // acc = (expression, i-th bit counter)
                if x == '1' {
                    (
                        format!(
                            "bv_left_shift({}, {}){}{}",
                            format!("to_xlen_t({}bv64)", acc.1),
                            expr,
                            if acc.0.len() == 0 { "" } else { " + " },
                            acc.0
                        ),
                        acc.1 + 1,
                    )
                } else {
                    (acc.0, acc.1 + 1)
                }
            })
            .0
    }
    fn gen_struct_defns(dwarf_reader: &Rc<DwarfReader<D>>) -> String {
        let mut defns = vec![];
        for var in dwarf_reader.global_vars() {
            defns.append(&mut Self::gen_struct_defn(&var.typ_defn));
        }
        for (_, func_sig) in dwarf_reader.func_sigs() {
            for var in &func_sig.args {
                defns.append(&mut Self::gen_struct_defn(&var.typ_defn));
            }
            if let Some(ret_typ) = &func_sig.ret_typ_defn {
                defns.append(&mut Self::gen_struct_defn(&ret_typ));
            }
        }
        defns.sort();
        defns.dedup();
        indent_text(format!("// Struct helpers\n{}", defns.join("\n")), 4)
    }
    fn gen_struct_defn(typ: &DwarfTypeDefn) -> Vec<String> {
        let mut defns = vec![];
        match typ {
            DwarfTypeDefn::Struct {
                id,
                fields,
                bytes: _,
            } => {
                for (field_name, field) in fields {
                    defns.append(&mut Self::gen_struct_defn(&*field.typ));
                    defns.push(format!(
                        "define {}(ptr: xlen_t): xlen_t = ptr + to_xlen_t({}bv64);",
                        Self::get_field_macro_name(&id[..], field_name),
                        field.loc
                    ));
                }
            }
            DwarfTypeDefn::Array { in_typ, out_typ } => {
                defns.append(&mut Self::gen_struct_defn(&in_typ));
                defns.append(&mut Self::gen_struct_defn(&out_typ));
            }
            _ => (),
        }
        defns
    }
    fn get_field_macro_name(struct_id: &str, field_name: &String) -> String {
        format!("{}_{}", struct_id, field_name)
    }
    fn gen_global_defns(dwarf_reader: &Rc<DwarfReader<D>>) -> String {
        let mut defns = String::from("// Global variables\n");
        for var in dwarf_reader.global_vars() {
            defns = format!("{}{}\n", defns, Self::gen_global_defn(&var));
        }
        indent_text(defns, 4)
    }
    fn gen_global_defn(global_var: &DwarfVar) -> String {
        format!(
            "define {}(): xlen_t = {};",
            Self::global_var_ptr_name(&global_var.name[..]),
            format!("to_xlen_t({}bv64)", global_var.memory_addr)
        )
    }
    fn global_var_ptr_name(name: &str) -> String {
        format!("global_{}", name)
    }
    fn gen_procs(model: &Model, dwarf_reader: &Rc<DwarfReader<D>>) -> String {
        let procs_string = model
            .func_models
            .iter()
            .map(|fm| Self::func_model_to_string(fm, dwarf_reader))
            .collect::<Vec<_>>()
            .join("\n\n");
        indent_text(procs_string, 4)
    }
    fn control_blk(model: &Model, dwarf_reader: &Rc<DwarfReader<D>>) -> String {
        let verif_fns_string = model
            .func_models
            .iter()
            .filter(|fm| dwarf_reader.func_sig(&fm.sig.name).is_some())
            .map(|fm| {
                format!(
                    "f{} = verify({});",
                    fm.sig.name.clone(),
                    fm.sig.name.clone()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let verif_fns_string = format!("{}\ncheck;\nprint_results;", verif_fns_string);
        let verif_fns_string = indent_text(verif_fns_string, 4);
        let control_string = format!("control {{\n{}\n}}", verif_fns_string);
        indent_text(control_string, 4)
    }
    /// Helper functions
    fn var_decl(var: &Var) -> String {
        format!(
            "{}: {}",
            Self::var_to_string(var),
            Self::typ_to_string(&var.typ)
        )
    }
}

impl<D> IRInterface for Uclid5Interface<D>
where
    D: DwarfInterface,
{
    type DwarfReader = DwarfReader<D>;
    /// IR translation functions
    fn lit_to_string(lit: &Literal) -> String {
        match lit {
            Literal::Bv { val, width } => format!("{}bv{}", *val as i64, width),
            Literal::Bool { val } => format!("{}", val),
        }
    }
    fn typ_to_string(typ: &Type) -> String {
        match typ {
            Type::Unknown => panic!("Type is unknown!"),
            Type::Bool => format!("bool"),
            Type::Bv { w } => format!("bv{}", w),
            Type::Array { in_typs, out_typ } => format!(
                "[{}]{}",
                in_typs
                    .iter()
                    .map(|typ| Self::typ_to_string(typ))
                    .collect::<Vec<_>>()
                    .join(", "),
                Self::typ_to_string(out_typ)
            ), // FIXME
        }
    }
    fn deref_app_to_string(bytes: u64, ptr: String, old: bool) -> String {
        format!("deref_{}({}(mem), {})", bytes, if old {"old"} else {""}, ptr)
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
            BVOp::Not => format!("~{}", e1.unwrap()),
            BVOp::UnaryMinus => format!("-{}", e1.unwrap()),
            BVOp::SignExt => {
                match e2.unwrap().split("bv").next().unwrap() {
                    width if width != "0" => format!("bv_sign_extend({}, {})", width, e1.unwrap()),
                    _ => format!("{}", e1.unwrap()),
                }
            },
            BVOp::ZeroExt => {
                match e2.unwrap().split("bv").next().unwrap() {
                    width if width != "0" => format!("bv_zero_extend({}, {})", width, e1.unwrap()),
                    _ => format!("{}", e1.unwrap()),
                }
            },
            BVOp::LeftShift => format!("bv_l_shift({}, {})", e2.unwrap(), e1.unwrap()),
            BVOp::Slice { l, r } => format!("{}[{}:{}]", e1.unwrap(), l - 1, r),
            _ => panic!("[bvop_to_string] Unimplemented."),
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
    fn fapp_to_string(fapp: &FuncApp) -> String {
        format!(
            "{}({})",
            fapp.func_name,
            fapp.operands
                .iter()
                .map(|x| Self::expr_to_string(&*x))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
    fn array_index_to_string(e1: String, e2: String) -> String {
        format!("{}[{}]", e1, e2)
    }
    fn get_field_to_string(e1: String, field: String) -> String {
        format!("{}.{}", e1, field)
    }

    /// Statements to string
    fn stmt_to_string(stmt: &Stmt) -> String {
        match stmt {
            Stmt::Skip => Self::skip_to_string(),
            Stmt::Assert(expr) => Self::assert_to_string(&expr),
            Stmt::Assume(expr) => Self::assume_to_string(&expr),
            Stmt::Havoc(var) => Self::havoc_to_string(var),
            Stmt::FuncCall(fc) => Self::func_call_to_string(&fc),
            Stmt::Assign(assign) => Self::assign_to_string(&assign),
            Stmt::IfThenElse(ite) => Self::ite_to_string(&ite),
            Stmt::Block(stmt_vec) => Self::block_to_string(&stmt_vec),
        }
    }
    fn skip_to_string() -> String {
        format!("")
    }
    fn assert_to_string(expr: &Expr) -> String {
        format!("assert ({});", Self::expr_to_string(expr))
    }
    fn assume_to_string(expr: &Expr) -> String {
        format!("assume ({});", Self::expr_to_string(expr))
    }
    fn havoc_to_string(var: &Rc<Var>) -> String {
        format!("havoc {};", Self::var_to_string(&*var))
    }
    fn func_call_to_string(func_call: &FuncCall) -> String {
        let lhs = func_call
            .lhs
            .iter()
            .map(|rc_expr| Self::expr_to_string(&*rc_expr))
            .collect::<Vec<_>>()
            .join(", ");
        let args = func_call
            .operands
            .iter()
            .map(|rc_expr| Self::expr_to_string(&*rc_expr))
            .collect::<Vec<_>>()
            .join(", ");
        format!("call ({}) = {}({});", lhs, func_call.func_name, args)
    }
    fn assign_to_string(assign: &Assign) -> String {
        let lhs = assign
            .lhs
            .iter()
            .map(|rc_expr| Self::expr_to_string(&*rc_expr))
            .collect::<Vec<_>>()
            .join(", ");
        let rhs = assign
            .rhs
            .iter()
            .map(|rc_expr| Self::expr_to_string(&*rc_expr))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} = {};", lhs, rhs)
    }
    fn ite_to_string(ite: &IfThenElse) -> String {
        let cond = Self::expr_to_string(&ite.cond);
        let thn = indent_text(Self::stmt_to_string(&*ite.then_stmt), 4);
        let els = if let Some(else_stmt) = &ite.else_stmt {
            format!(
                "else {{\n{}\n}}",
                indent_text(Self::stmt_to_string(&*else_stmt), 4)
            )
        } else {
            String::from("")
        };
        format!("if ({}) {{\n{}\n}}{}", cond, thn, els)
    }
    fn block_to_string(blk: &Vec<Box<Stmt>>) -> String {
        let inner = blk
            .iter()
            .map(|rc_stmt| Self::stmt_to_string(rc_stmt))
            .collect::<Vec<_>>()
            .join("\n");
        let inner = indent_text(inner, 4);
        format!("{{\n{}\n}}", inner)
    }
    fn func_model_to_string(fm: &FuncModel, dwarf_reader: &Rc<Self::DwarfReader>) -> String {
        let args = fm
            .sig
            .arg_decls
            .iter()
            .map(|var| Self::var_decl(&var.get_expect_var()))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = if let Some(rd) = &fm.sig.ret_decl {
            format!("returns ({})", Self::var_decl(rd.get_expect_var()))
        } else {
            format!("")
        };
        let requires = fm
            .sig
            .requires
            .iter()
            .map(|spec| {
                format!(
                    "\n    requires ({});",
                    Self::spec_expr_to_string(&fm.sig.name[..], spec.expr(), dwarf_reader, spec.expr().contains_old())
                )
            })
            .collect::<Vec<_>>()
            .join("");
        let ensures = fm
            .sig
            .ensures
            .iter()
            .map(|spec| {
                format!(
                    "\n    ensures ({});",
                    Self::spec_expr_to_string(&fm.sig.name[..], spec.expr(), dwarf_reader, spec.expr().contains_old())
                )
            })
            .collect::<Vec<_>>()
            .join("");
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
        let body = Self::block_to_string(fm.body.get_expect_block());
        let inline = if fm.inline { "[inline] " } else { "" };
        format!(
            "procedure {}{}({}){}{}{}{}\n{}",
            inline, fm.sig.name, args, ret, modifies, requires, ensures, body
        )
    }

    // Generate function model
    // NOTE: Replace string with write to file
    fn model_to_string(xlen: &u64, model: &Model, dwarf_reader: Rc<Self::DwarfReader>) -> String {
        let xlen_defn = indent_text(
            format!(
                "type xlen_t = bv{};\ndefine to_xlen_t(x: bv64): xlen_t = x[{}:0];",
                xlen,
                xlen - 1
            ),
            4,
        );
        // prelude
        let prelude = Self::prelude();
        // variables
        let var_defns = indent_text(Self::gen_var_defns(model), 4);
        // definitions
        let array_defns = Self::gen_array_defns(&dwarf_reader);
        let struct_defns = Self::gen_struct_defns(&dwarf_reader);
        let global_defns = Self::gen_global_defns(&dwarf_reader);
        // procedures
        let procs = Self::gen_procs(model, &dwarf_reader);
        // control block
        let ctrl_blk = Self::control_blk(model, &dwarf_reader);
        format!(
            "module main {{\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n\n{}\n}}",
            xlen_defn, prelude, var_defns, array_defns, struct_defns, global_defns, procs, ctrl_blk
        )
    }

    /// Specification langauge translation functions
    fn spec_fapp_to_string(
        name: &str,
        fapp: &FuncApp,
        dwarf_reader: &Rc<Self::DwarfReader>,
    ) -> String {
        format!(
            "{}({})",
            fapp.func_name,
            fapp.operands
                .iter()
                .map(|x| Self::spec_expr_to_string(name, &*x, dwarf_reader, false))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
    fn spec_opapp_to_string(
        func_name: &str,
        opapp: &OpApp,
        dwarf_reader: &Rc<Self::DwarfReader>,
        old: bool,
    ) -> String {
        let e1_str = opapp.operands.get(0).map_or(None, |e| {
            Some(Self::spec_expr_to_string(func_name, e, dwarf_reader, old))
        });
        let e2_str = opapp.operands.get(1).map_or(None, |e| {
            Some(Self::spec_expr_to_string(func_name, e, dwarf_reader, old))
        });
        match &opapp.op {
            Op::Deref => {
                let typ = Self::get_expr_type(
                    func_name,
                    opapp.operands.get(0).unwrap(),
                    &dwarf_reader.typ_map(),
                );
                let bytes = typ.to_bytes();
                Self::deref_app_to_string(bytes, e1_str.unwrap(), old)
            }
            // FIXME: Add old flag to all spec functions and wrap "old"
            // around the base identifier or memory variable if it's a global
            Op::Old => Self::spec_expr_to_string(func_name, opapp.operands.get(0).expect("Old operator is missing an expression."), dwarf_reader, true),
            Op::Comp(cop) => Self::comp_app_to_string(cop, e1_str, e2_str),
            Op::Bv(bvop) => Self::bv_app_to_string(bvop, e1_str, e2_str),
            Op::Bool(bop) => Self::bool_app_to_string(bop, e1_str, e2_str),
            Op::ArrayIndex => {
                // Get expression expression type
                let typ = Self::get_expr_type(
                    func_name,
                    opapp.operands.get(0).unwrap(),
                    &dwarf_reader.typ_map(),
                );
                let typ_size = match &*typ {
                    DwarfTypeDefn::Array { in_typ: _, out_typ }
                    | DwarfTypeDefn::Pointer { value_typ: out_typ } => out_typ.as_ref().to_bytes(),
                    // FIXME: REMOVE CASE
                    // DwarfTypeDefn::Primitive { bytes } => bytes,
                    _ => panic!("Should be array or pointer type!"),
                };
                let array = e1_str.unwrap();
                let index = e2_str.unwrap();
                format!(
                    "{}({}, {})",
                    Self::array_index_macro_name(&typ_size),
                    array,
                    index
                )
            }
            Op::GetField(field) => {
                let struct_id = "SID";
                format!(
                    "{}({})",
                    Self::get_field_macro_name(struct_id, field),
                    e1_str.unwrap()
                )
            }
        }
    }

    /// Specification variable to Uclid5 variable
    /// Globals are shadowed by function variables
    fn spec_var_to_string(
        func_name: &str,
        v: &Var,
        dwarf_reader: &Rc<Self::DwarfReader>,
        old: bool,
    ) -> String {
        if v.name.chars().next().unwrap() == '$' {
            let name = v.name.replace("$", "");
            if name == "ret" {
                let typ = Self::get_expr_type(
                    func_name,
                    &Expr::var(&v.name, Type::Unknown),
                    &dwarf_reader.typ_map(),
                );
                format!("{}(a0)[{}:0]", if old {"old"} else {""}, BYTE_SIZE * typ.to_bytes() - 1)
            } else {
                format!("{}({})", if old {"old"} else {""}, name)
            }
        } else if dwarf_reader
            .func_sigs()
            .iter()
            .find(|(_, fs)| fs.args.iter().find(|arg| arg.name == v.name).is_some())
            .is_some()
            || vec![
                translator::PC_VAR,
                translator::MEM_VAR,
                translator::PRIV_VAR,
                translator::EXCEPT_VAR,
            ]
            .contains(&&v.name[..])
        {
            format!("{}({})", if old {"old"} else {""}, v.name.clone())
        } else if dwarf_reader
            .global_vars()
            .iter()
            .find(|x| x.name == v.name)
            .is_some()
        {
            format!("{}()", Self::global_var_ptr_name(&v.name[..]))
        } else {
            panic!("Unable to find variable {:?}", v);
        }
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
