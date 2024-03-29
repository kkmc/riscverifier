use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use crate::veriv_ast as ast;

// ==================================================================
/// # AST Types

#[derive(Debug, Clone, PartialEq)]
pub enum VType {
    Unknown,
    Bv(u16),
    Int,
    Bool,
    Array {
        in_type: Box<VType>,
        out_type: Box<VType>,
    },
    Struct {
        id: String,
        fields: HashMap<String, Box<VType>>,
        size: u64,
    },
}
impl VType {
    /// Returns the output type of an array
    pub fn get_array_out_type(&self) -> &VType {
        match self {
            Self::Array { in_type:_, out_type } => &*out_type,
            _ => panic!("Not an array type; found {:?}", self),
        }
    }

    /// Returns the output type of the array
    pub fn get_array_out_type_size(&self) -> u64 {
        match self {
            Self::Array { in_type:_, out_type } => {
                match **out_type {
                    Self::Bv(w) => w as u64,
                    Self::Struct { id:_, fields:_, size } => size,
                    _ => panic!("Array has invalid output type {:?}.", self),
                }
            },
            _ => panic!("Not an array type; found {:?}.", self),
        }
    }

    /// Returns the width of the type
    pub fn get_bv_width(&self) -> u16 {
        match self {
            Self::Bv(width) => *width,
            _ => panic!("Not a bv type: {:#?}", self),
        }
    }
    
    /// Infer the operator type based on the `ValueOp` and operands `exprs`
    pub fn infer_op_type(op: &ValueOp, exprs: &Vec<VExpr>) -> VType {
        if exprs.len() == 0 {
            panic!("Operator with no arguments provided.");
        }
        match op {
            ValueOp::ArrayIndex => match exprs[0].typ() {
                Self::Array {
                    in_type: _,
                    out_type,
                } => *out_type.clone(),
                _ => panic!("ArrayIndex should have an array typed first argument."),
            },
            ValueOp::Slice { lo, hi } => Self::Bv(hi - lo),
            ValueOp::GetField => match &exprs[0].typ() {
                Self::Struct {
                    id,
                    fields,
                    size: _,
                } => match &exprs[1] {
                    VExpr::Ident(name, _) => {
                        if let Some(box_typ) = fields.get(name) {
                            *box_typ.clone()
                        } else {
                            panic!("Invalid struct field: {} is not a field of {}.", name, id)
                        }
                    }
                    _ => panic!("Field of GetField operator should be an identifier."),
                },
                _ => panic!("GetField should have a struct typed first argument."),
            },
            ValueOp::Add
            | ValueOp::Sub
            | ValueOp::Div
            | ValueOp::Mul
            | ValueOp::BvXor
            | ValueOp::BvOr
            | ValueOp::BvAnd
            | ValueOp::Deref => {
                // These operators require all the same types
                let same_types = exprs
                    .iter()
                    .fold(true, |acc, expr| acc && exprs[0].typ() == expr.typ());
                if same_types {
                    exprs[0].typ().clone()
                } else {
                    panic!("Expected the same types. {:?}", exprs)
                }
            }
            ValueOp::Concat => {
                let width0 = exprs[0].typ().get_bv_width();
                let width1 = exprs[1].typ().get_bv_width();
                Self::Bv(width0 + width1)
            }
            ValueOp::RightShift | ValueOp::URightShift | ValueOp::LeftShift => {
                exprs[1].typ().clone()
            }
        }
    }
    pub fn infer_func_app_type(fapp: &str, exprs: &Vec<VExpr>) -> VType {
        if exprs.len() == 0 {
            panic!("Function application with no arguments provided.");
        }
        match fapp {
            "old" | "value" => exprs[0].typ().clone(),
            "sext" | "uext" => {
                let expr_width = exprs[1].typ().get_bv_width();
                let ext_width = exprs[0].get_lit_value().expect("Expected literal value.") as u16;
                Self::Bv(expr_width + ext_width)
            }
            _ => panic!("Unimplemented type inference for {}.", fapp),
        }
    }

    /// TODO: Replace this and above with generic and have each AST type implement a type trait
    pub fn from_ast_type(typ: &ast::Type) -> Self {
        match typ {
            ast::Type::Unknown => Self::Unknown,
            ast::Type::Bool => Self::Bool,
            ast::Type::Int => Self::Int,
            ast::Type::Bv { w } => Self::Bv(*w as u16),
            ast::Type::Array { in_typs, out_typ } => {
                let in_type = Box::new(Self::from_ast_type(&in_typs[0]));
                let out_type = Box::new(Self::from_ast_type(&out_typ));
                Self::Array { in_type, out_type }
            }
            ast::Type::Struct { id, fields, w } => {
                let id = id.clone();
                let fields = fields
                    .iter()
                    .map(|kv| {
                        let field_name = (&*kv.0).clone();
                        let field_type = Self::from_ast_type(&*kv.1);
                        (field_name, Box::new(field_type))
                    })
                    .collect();
                let size = *w;
                Self::Struct { id, fields, size }
            }
        }
    }
}

// ==================================================================
/// # AST Expressions

// Boolean expression
#[derive(Debug, Clone)]
pub enum BExpr {
    Bool(bool),
    // Boolean operator application
    BOpApp(BoolOp, Vec<BExpr>),
    // Comparison operator application
    COpApp(CompOp, Vec<VExpr>),
}

#[derive(Debug, Clone)]
pub enum BoolOp {
    Conj,                 // &&
    Disj,                 // ||
    Neg,                  // !
    Implies,              // ==>
    Forall(VExpr, VType), // forall
    Exists(VExpr, VType), // exists
}

#[derive(Debug, Clone)]
pub enum CompOp {
    Equal,  // ==
    Nequal, // !=
    Gt,     // >
    Lt,     // <
    Gtu,    // >_u
    Ltu,    // <_u
    Geq,    // >=
    Leq,    // <=
    Geu,    // >=_u
    Leu,    // <=_u
}

// Value expression
#[derive(Debug, Clone)]
pub enum VExpr {
    Bv { value: u64, typ: VType },
    Int(i64, VType),
    Bool(bool, VType),
    Ident(String, VType),
    OpApp(ValueOp, Vec<VExpr>, VType),
    FuncApp(String, Vec<VExpr>, VType),
}
impl VExpr {
    /// Returns the type of the value expression
    /// based on the dwarf context
    /// If no dwarf context is provided, types are unknown
    /// for variables and expressions consisting of those variables.
    /// Type inference is currently not implemented except for the
    /// usual bottom up type propagation (e.g. int + int => int).
    pub fn typ(&self) -> &VType {
        match self {
            Self::Bv { value: _, typ }
            | Self::Int(_, typ)
            | Self::Bool(_, typ)
            | Self::Ident(_, typ)
            | Self::OpApp(_, _, typ)
            | Self::FuncApp(_, _, typ) => typ,
        }
    }

    /// Helper function that returns if the value is a literal
    pub fn is_lit(&self) -> bool {
        match self {
            Self::Bv { value:_ , typ:_ } |
            Self::Int(_, _) |
            Self::Bool(_, _) => true,
            _ => false,
        }
    }

    /// Helper function that returns the value of a bitvector VExpr
    pub fn get_lit_value(&self) -> Option<u64> {
        match self {
            Self::Bv { value, typ:_ } => Some(*value),
            Self::Int(value, _) => Some(*value as u64),
            Self::Bool(b, _) => Some(if *b { 1 } else { 0 }),
            _ => None,
        }
    }

    /// Helper function that returns the identifier name as a string
    pub fn get_ident_name(&self) -> &str {
        match self {
            Self::Ident(name, _) => name,
            _ => panic!("Expected `Self::Ident` but found {:?}.", self),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ValueOp {
    Add,                        // +
    Sub,                        // -
    Div,                        // /
    Mul,                        // *
    BvXor,                      // ^
    BvOr,                       // |
    BvAnd,                      // &
    RightShift,                 // >>
    URightShift,                // >>>
    LeftShift,                  // <<
    ArrayIndex,                 // a[i]
    GetField,                   // s.f
    Slice { lo: u16, hi: u16 }, // a[lo:hi]
    Concat,
    Deref,
}

#[derive(Debug, Clone)]
pub enum Spec {
    Requires(BExpr),
    Ensures(BExpr),
    Modifies(HashSet<String>),
    Track(String, VExpr),
}
impl Spec {
    pub fn get_bexpr(&self) -> Result<&BExpr, ()> {
        match self {
            Self::Requires(e) => Ok(e),
            Self::Ensures(e) => Ok(e),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FuncSpec {
    pub fname: String,
    pub specs: Vec<Spec>,
}

// ==================================================================
/// # AST Rewriter

pub trait ASTRewriter<C> {
    fn rewrite_bexpr(bexpr: BExpr, _ctx: &RefCell<C>) -> BExpr { bexpr }
    fn rewrite_bexpr_bool(bool_expr: BExpr, _ctx: &RefCell<C>) -> BExpr { bool_expr }
    fn rewrite_bexpr_bopapp(bopapp: BExpr, _ctx: &RefCell<C>) -> BExpr { bopapp }
    fn rewrite_bexpr_copapp(copapp: BExpr, _ctx: &RefCell<C>) -> BExpr { copapp }
    fn rewrite_bexpr_boolop(bop: BoolOp, _ctx: &RefCell<C>) -> BoolOp { bop }
    fn rewrite_bexpr_compop(cop: CompOp, _ctx: &RefCell<C>) -> CompOp { cop }

    // BExpr
    fn visit_bexpr(bexpr: BExpr, context: &RefCell<C>) -> BExpr {
        let rw_bexpr = match bexpr {
            BExpr::Bool(_) => Self::visit_bexpr_bool(bexpr, context),
            BExpr::BOpApp(_, _) => Self::visit_bexpr_bopapp(bexpr, context),
            BExpr::COpApp(_, _) => Self::visit_bexpr_copapp(bexpr, context),
        };
        Self::rewrite_bexpr(rw_bexpr, context)
    }

    fn visit_bexpr_bool(bool_expr: BExpr, context: &RefCell<C>) -> BExpr {
        Self::rewrite_bexpr_bool(bool_expr, context)
    }
    
    fn visit_bexpr_bopapp(bopapp: BExpr, context: &RefCell<C>) -> BExpr {
        let rw_bopapp = match bopapp {
            BExpr::BOpApp(bop, exprs) => {
                let rw_bop = Self::visit_bexpr_boolop(bop, context);
                let rw_bexprs = Self::visit_bexprs(exprs, context);
                BExpr::BOpApp(rw_bop, rw_bexprs)
            }
            _ => panic!("Impleemntation error; expected `BExpr::BOpApp`.")
        };
        Self::rewrite_bexpr_bopapp(rw_bopapp, context)
    }

    fn visit_bexpr_copapp(copapp: BExpr, context: &RefCell<C>) -> BExpr {
        let rw_copapp = match copapp {
            BExpr::COpApp(cop, exprs) => {
                let rw_cop = Self::visit_bexpr_compop(cop, context);
                let rw_vexprs = Self::visit_vexprs(exprs, context);
                BExpr::COpApp(rw_cop, rw_vexprs)
            }
            _ => panic!("Implemntation error; expected `BExpr::COpApp`.")
        };
        Self::rewrite_bexpr_copapp(rw_copapp, context)
    }

    fn visit_bexpr_boolop(bop: BoolOp, context: &RefCell<C>) -> BoolOp {
        Self::rewrite_bexpr_boolop(bop, context)
    }
    
    fn visit_bexpr_compop(cop: CompOp, context: &RefCell<C>) -> CompOp {
        Self::rewrite_bexpr_compop(cop, context)
    }
    
    fn visit_bexprs(exprs: Vec<BExpr>, context: &RefCell<C>) -> Vec<BExpr> {
        exprs.into_iter().map(|expr| Self::visit_bexpr(expr, context)).collect::<Vec<_>>()
    }

    fn rewrite_vexpr(vexpr: VExpr, _ctx: &RefCell<C>) -> VExpr { vexpr }
    fn rewrite_vexpr_bvvalue(value: VExpr, _ctx: &RefCell<C>) -> VExpr { value }
    fn rewrite_vexpr_int(i: VExpr, _ctx: &RefCell<C>) -> VExpr { i }
    fn rewrite_vexpr_bool(b: VExpr, _ctx: &RefCell<C>) -> VExpr { b }
    fn rewrite_vexpr_ident(ident: VExpr, _ctx: &RefCell<C>) -> VExpr { ident }
    fn rewrite_vexpr_opapp(opapp: VExpr, _ctx: &RefCell<C>) -> VExpr { opapp }
    fn rewrite_vexpr_funcapp(funcapp: VExpr, _ctx: &RefCell<C>) -> VExpr { funcapp }
    fn rewrite_vexpr_valueop(vop: ValueOp, _ctx: &RefCell<C>) -> ValueOp { vop }
    fn rewrite_vexpr_funcid(fid: String, _ctx: &RefCell<C>) -> String { fid }
    
    // VExpr
    fn visit_vexpr(vexpr: VExpr, context: &RefCell<C>) -> VExpr {
        let rw_vexpr = match vexpr {
            VExpr::Bv { value: _, typ: _ } => Self::visit_vexpr_bvvalue(vexpr, context),
            VExpr::Int(_, _) => Self::visit_vexpr_int(vexpr, context),
            VExpr::Bool(_, _) => Self::visit_vexpr_bool(vexpr, context),
            VExpr::Ident(_, _) => Self::visit_vexpr_ident(vexpr, context),
            VExpr::OpApp(_, _, _) => Self::visit_vexpr_opapp(vexpr, context),
            VExpr::FuncApp(_, _, _) => Self::visit_vexpr_funcapp(vexpr, context),
        };
        Self::rewrite_vexpr(rw_vexpr, context)
    }
    
    fn visit_vexprs(exprs: Vec<VExpr>, context: &RefCell<C>) -> Vec<VExpr> {
        exprs.into_iter().map(|expr| Self::visit_vexpr(expr, context)).collect::<Vec<_>>()
    }
    
    fn visit_vexpr_bvvalue(value: VExpr, context: &RefCell<C>) -> VExpr {
        Self::rewrite_vexpr_bvvalue(value, context)
    }
    
    fn visit_vexpr_int(i: VExpr, context: &RefCell<C>) -> VExpr {
        Self::rewrite_vexpr_int(i, context)
    }
    
    fn visit_vexpr_bool(b: VExpr, context: &RefCell<C>) -> VExpr {
        Self::rewrite_vexpr_bool(b, context)
    }
    
    fn visit_vexpr_ident(vexpr: VExpr, context: &RefCell<C>) -> VExpr {
        Self::rewrite_vexpr_ident(vexpr, context)
    }
    
    fn visit_vexpr_opapp(opapp: VExpr, context: &RefCell<C>) -> VExpr {
        let rw_vexpr_opapp = match opapp {
            VExpr::OpApp(op, exprs, typ) => {
                let rw_op = Self::visit_vexpr_valueop(op, context);
                let rw_vexprs = Self::visit_vexprs(exprs, context);
                VExpr::OpApp(rw_op, rw_vexprs, typ)
            }
            _ => panic!("Implementation error; expected `VExpr::OpApp`."),
        };
        Self::rewrite_vexpr_opapp(rw_vexpr_opapp, context)
    }
    
    fn visit_vexpr_funcapp(funcapp: VExpr, context: &RefCell<C>) -> VExpr {
        let rw_vexpr = match funcapp {
            VExpr::FuncApp(fid, exprs, typ) => {
                let rw_fid = Self::visit_vexpr_funcid(fid, context);
                let rw_vexprs = Self::visit_vexprs(exprs, context);
                VExpr::FuncApp(rw_fid, rw_vexprs, typ)
            }
            _ => panic!("Implementation error; expected `VExpr::FuncApp`."),
        };
        Self::rewrite_vexpr_funcapp(rw_vexpr, context)
    }
    
    fn visit_vexpr_valueop(vop: ValueOp, context: &RefCell<C>) -> ValueOp {
        Self::rewrite_vexpr_valueop(vop, context)
    }
    
    fn visit_vexpr_funcid(fid: String, context: &RefCell<C>) -> String {
        Self::rewrite_vexpr_funcid(fid, context)
    }
}
