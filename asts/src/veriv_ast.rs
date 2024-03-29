use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    fmt,
    cell::RefCell,
    hash::Hash,
};

use crate::spec_lang::sl_ast;

// =======================================================
/// # VERI-V IR AST

// =======================================================
/// ## AST Types

#[derive(PartialEq, Eq, Hash, Clone)]
pub enum Type {
    Unknown,
    Bool,
    Int,
    Bv {
        w: u64,
    },
    Array {
        in_typs: Vec<Box<Type>>,
        out_typ: Box<Type>,
    },
    Struct {
        id: String,
        fields: BTreeMap<String, Box<Type>>,
        w: u64,
    },
}

impl Type {
    pub fn mk_unknown_type() -> Self {
        Type::Unknown
    }
    pub fn mk_bool_type() -> Self {
        Type::Bool
    }
    pub fn mk_int_type() -> Self {
        Type::Int
    }
    pub fn mk_bv_type(w: u64) -> Self {
        Type::Bv {
            w
        }
    }
    pub fn mk_array_type(in_typs: Vec<Box<Type>>, out_typ: Box<Type>) -> Self {
        Type::Array {
            in_typs,
            out_typ
        }
    }
    pub fn mk_struct_type(id: String, fields: BTreeMap<String, Box<Type>>, w: u64) -> Self {
        Type::Struct {
            id, fields, w
        }
    }
    pub fn get_expect_bv_width(&self) -> u64 {
        match self {
            Type::Bv { w } => *w,
            Type::Struct {
                id: _,
                fields: _,
                w,
            } => *w,
            _ => panic!("No bv width for: {}.", self),
        }
    }
    pub fn get_array_out_type(&self) -> &Type {
        match self {
            Type::Array {
                in_typs: _,
                out_typ,
            } => out_typ,
            _ => panic!("Not an array type: {}.", self),
        }
    }
    pub fn get_struct_id(&self) -> String {
        match self {
            Type::Struct {
                id,
                fields: _,
                w: _,
            } => id.clone(),
            _ => panic!("Not a struct type {}.", self),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Unknown => write!(f, "Unknown"),
            Type::Bool => write!(f, "bool"),
            Type::Int => write!(f, "int"),
            Type::Bv { w } => write!(f, "bv{}", w),
            Type::Array { in_typs, out_typ } => {
                let in_typs = &in_typs
                    .iter()
                    .fold("".to_string(), |acc, typ| format!("{}, {}", acc, typ))[2..];
                write!(f, "[{}]{}", in_typs, out_typ)
            }
            Type::Struct {
                id,
                fields: _,
                w: _,
            } => write!(f, "struct {}", id),
        }
    }
}

// =======================================================
/// ## AST Expressions

#[derive(Hash, PartialEq, Eq, Clone)]
pub enum Expr {
    Literal(Literal, Type),
    Var(Var, Type),
    OpApp(OpApp, Type),
    FuncApp(FuncApp, Type),
}

impl Expr {
    /// Returns the type of the expression.
    pub fn typ(&self) -> &Type {
        match self {
            Expr::Literal(_, t) | Expr::Var(_, t) | Expr::OpApp(_, t) | Expr::FuncApp(_, t) => &t,
        }
    }

    /// Returns the variable name or panics if it's not a variable.
    pub fn get_var_name(&self) -> String {
        match self {
            Expr::Var(v, _) => v.name.clone(),
            _ => panic!("Not a variable/constant: {}.", self),
        }
    }

    /// Returns whether or not the expression is a variable.
    pub fn is_var(&self) -> bool {
        if let Expr::Var(_, _) = self {
            true
        } else {
            false
        }
    }

    /// Returns whether the expression is a literal
    pub fn is_lit(&self) -> bool {
        match self {
            Self::Literal(_, _) => true, 
            _ => false,
        }
    }

    /// Returns the literal inside Expr::Literal
    pub fn get_lit(&self) -> Option<&Literal> {
        match self {
            Self::Literal(lit, _) => Some(lit),
            _ => None,
        }
    }

    /// Returns the value of the literal
    /// Boolean values are encoded as 0 (false) and 1 (true)
    pub fn get_lit_value(&self) -> Option<u64> {
        let lit_opt = Self::get_lit(self);
        lit_opt.map(|lit| {
            match lit {
                Literal::Bv { val, width: _ } => *val,
                Literal::Bool { val } => if *val { 1 } else { 0 },
                Literal::Int { val } => *val,
            }
        })
    }

    /// Returns the bv width for bv literals
    pub fn get_expect_bv_width(&self) -> u64 {
        match self {
            Expr::Literal(lit, _) => lit.get_expect_bv_width(),
            _ => panic!("Expression is not bv type."),
        }
    }

    /// Returns the array variable expression if it is an array access
    pub fn get_array_expr(&self) -> Option<&Expr> {
        match self {
            Expr::OpApp(opapp, _) => opapp.get_array_expr(),
            _ => None,
        }
    }

    /// Returns the array index of the array access
    pub fn get_array_index(&self) -> Option<&Expr> {
        match self {
            Expr::OpApp(opapp, _) => opapp.get_array_index(),
            _ => None
        }
    }

    /// Returns a bitvector literal of value `val` and width `width`.
    pub fn bv_lit(val: u64, width: u64) -> Self {
        Expr::Literal(Literal::Bv { val, width }, Type::Bv { w: width })
    }

    /// Returns a integer literal of value `val`.
    pub fn int_lit(val: u64) -> Self {
        Expr::Literal(Literal::Int { val }, Type::Int)
    }

    /// Returns a boolean literal of value `val`.
    pub fn bool_lit(val: bool) -> Self {
        Expr::Literal(Literal::Bool { val }, Type::Bool)
    }

    /// Creates a variable named `name` of type `typ`.
    pub fn var(name: &str, typ: Type) -> Self {
        Expr::Var(
            Var {
                name: name.to_string(),
                typ: typ.clone(),
            },
            typ.clone(),
        )
    }

    /// Create an operator application expression.
    pub fn op_app(op: Op, operands: Vec<Self>) -> Self {
        let typ = match &op {
            Op::Comp(_) | Op::Bool(_) => Type::Bool,
            Op::Bv(_) => operands[0].typ().clone(),
            Op::ArrayIndex => match operands[0].typ() {
                Type::Array {
                    in_typs: _,
                    out_typ,
                } => *out_typ.clone(),
                _ => panic!("Cannot index into non-array type {}.", operands[0]),
            },
            Op::GetField(f) => match operands[0].typ() {
                Type::Struct {
                    id: _,
                    fields,
                    w: _,
                } => *fields.get(f).expect("Invalid field.").clone(),
                _ => panic!("Can only get field from struct type."),
            },
        };
        Expr::OpApp(OpApp { op, operands }, typ)
    }

    /// Creates a function application expression.
    pub fn func_app(func_name: String, operands: Vec<Self>, typ: Type) -> Self {
        Expr::FuncApp(
            FuncApp {
                func_name,
                operands,
            },
            typ,
        )
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Literal(l, _) => write!(f, "{}", l),
            Expr::Var(v, _) => write!(f, "{}", v),
            Expr::OpApp(op, _) => write!(f, "{}", op),
            Expr::FuncApp(fapp, _) => write!(f, "{}", fapp),
        }
    }
}

/// Literals
#[derive(PartialEq, Eq, Hash, Clone)]
pub enum Literal {
    Bv { val: u64, width: u64 },
    Bool { val: bool },
    Int { val: u64 },   // TODO: Change this to i64
}

impl Literal {
    fn get_expect_bv_width(&self) -> u64 {
        match self {
            Self::Bv { val: _, width } => *width,
            _ => panic!("Tried to get bv width from non-bv literal.")
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Bv { val, width } => write!(f, "{}bv{}", val, width),
            Literal::Bool { val } => write!(f, "{}", val),
            Literal::Int { val } => write!(f, "{}", val),
        }
    }
}

/// Variable
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Var {
    pub name: String,
    pub typ: Type,
}

impl Ord for Var {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

impl PartialOrd for Var {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Var {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

// Operator application
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct OpApp {
    pub op: Op,
    pub operands: Vec<Expr>,
}

impl OpApp {
    /// Returns the array variable of the array access
    pub fn get_array_expr(&self) -> Option<&Expr> {
        match self.op {
            Op::ArrayIndex => self.operands.get(0),
            _ => None,
        }
    }

    /// Returns the array index of the array access
    pub fn get_array_index(&self) -> Option<&Expr> {
        match self.op {
            Op::ArrayIndex => self.operands.get(1),
            _ => None,
        }
    }
}

impl fmt::Display for OpApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operands = &self.operands.iter().fold("".to_string(), |acc, operand| {
            format!("{}, {}", acc, operand)
        })[2..];
        write!(f, "({:?} {})", self.op, operands)
    }
}

/// Operators
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum Op {
    Comp(CompOp),
    Bv(BVOp),
    Bool(BoolOp),
    ArrayIndex,
    GetField(String),
}

/// Comparison operators
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum CompOp {
    Equality,
    Inequality,
    Lt,  // <
    Le,  // <=
    Gt,  // >
    Ge,  // >=
    Ltu, // <_u (unsigned)
    Leu, // <=_u
    Gtu, // >_u
    Geu, // >=_u
}

/// BV operators
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum BVOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    SignExt,
    ZeroExt,
    LeftShift,
    RightShift,
    ARightShift, // arithmetic right shift
    Concat,
    Slice { l: u64, r: u64 },
}

/// Boolean operators
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum BoolOp {
    Conj, // and: &&
    Disj, // or: ||
    Iff,  // if and only if: <==>
    Impl, // implication: ==>
    Neg,  // negation: !
}

/// Function application
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct FuncApp {
    pub func_name: String,
    pub operands: Vec<Expr>,
}

impl fmt::Display for FuncApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operands = &self.operands.iter().fold("".to_string(), |acc, operand| {
            format!("{}, {}", acc, operand)
        })[2..];
        write!(f, "{}({})", self.func_name, operands)
    }
}

// =======================================================
/// ## AST Expression Visitor

pub trait ASTRewriter<C> {
    fn rewrite_stmt(stmt: Stmt, _ctx: &RefCell<C>) -> Stmt { stmt }
    fn rewrite_stmt_assume(stmt: Stmt, _ctx: &RefCell<C>) -> Stmt { stmt }
    fn rewrite_funccall(fc: FuncCall, _ctx: &RefCell<C>) -> FuncCall { fc }
    fn rewrite_assign(a: Assign, _ctx: &RefCell<C>) -> Assign { a }
    fn rewrite_ite(ite: IfThenElse, _ctx: &RefCell<C>) -> IfThenElse { ite }
    fn rewrite_stmt_block(blk: Stmt, _ctx: &RefCell<C>) -> Stmt { blk }

    fn rewrite_type(typ: Type, _ctx: &RefCell<C>) -> Type { typ }
    
    fn rewrite_expr(expr: Expr, _ctx: &RefCell<C>) -> Expr { expr }
    fn rewrite_lit(lit: Literal, _ctx: &RefCell<C>) -> Literal { lit }
    fn rewrite_var(var: Var, _ctx: &RefCell<C>) -> Var { var }
    fn rewrite_opapp(opapp: OpApp, _ctx: &RefCell<C>) -> OpApp { opapp }
    fn rewrite_op(op: Op, _ctx: &RefCell<C>) -> Op { op }
    fn rewrite_funcapp(fapp: FuncApp, _ctx: &RefCell<C>) -> FuncApp { fapp }

    // Statement rewriters
    fn visit_stmt(stmt: Stmt, ctx: &RefCell<C>) -> Stmt {
        let rw_stmt = match stmt {
            Stmt::Assume(_) => Self::visit_stmt_assume(stmt, ctx),
            Stmt::FuncCall(_) => Self::visit_stmt_funccall(stmt, ctx),
            Stmt::Assign(_) => Self::visit_stmt_assign(stmt, ctx),
            Stmt::IfThenElse(_) => Self::visit_stmt_ifthenelse(stmt, ctx),
            Stmt::Block(_) => Self::visit_stmt_block(stmt, ctx),
            Stmt::Comment(_) => stmt,
        };
        Self::rewrite_stmt(rw_stmt, ctx)
    }
    fn visit_stmt_assume(stmt: Stmt, ctx: &RefCell<C>) -> Stmt {
        let rw_stmt = match stmt {
            Stmt::Assume(e) => Stmt::Assume(Self::visit_expr(e, ctx)),
            _ => panic!("Implementation error; Expected assume statement."),
        };
        Self::rewrite_stmt_assume(rw_stmt, ctx)
    }
    fn visit_expr(expr: Expr, ctx: &RefCell<C>) -> Expr {
        let rw_expr = match expr {
            Expr::Literal(_, _) => Self::visit_expr_lit(expr, ctx),
            Expr::Var(_, _) => Self::visit_expr_var(expr, ctx),
            Expr::OpApp(_, _) => Self::visit_expr_opapp(expr, ctx),
            Expr::FuncApp(_, _) => Self::visit_expr_funcapp(expr, ctx),
        };
        Self::rewrite_expr(rw_expr, ctx)
    }
    fn visit_type(typ: Type, ctx: &RefCell<C>) -> Type {
        Self::rewrite_type(typ, ctx)
    }
    fn visit_expr_lit(expr: Expr, ctx: &RefCell<C>) -> Expr {
        match expr {
            Expr::Literal(lit, typ) => {
                let rw_lit = Self::visit_lit(lit, ctx);
                let rw_typ = Self::visit_type(typ, ctx);
                Expr::Literal(rw_lit, rw_typ)
            }
            _ => panic!("Implementation error; Expected literal."),
        }
    }
    fn visit_lit(lit: Literal, ctx: &RefCell<C>) -> Literal {
        Self::rewrite_lit(lit, ctx)
    }
    fn visit_expr_var(expr: Expr, ctx: &RefCell<C>) -> Expr {
        match expr {
            Expr::Var(var, typ) => {
                let rw_var = Self::visit_var(var, ctx);
                let rw_typ = Self::visit_type(typ, ctx);
                Expr::Var(rw_var, rw_typ)
            }
            _ => panic!("Implementation error; Expected var."),
        }

    }
    fn visit_var(var: Var, ctx: &RefCell<C>) -> Var {
        Self::rewrite_var(var, ctx)
    }
    fn visit_expr_opapp(expr: Expr, ctx: &RefCell<C>) -> Expr {
        match expr {
            Expr::OpApp(opapp, typ) => {
                let rw_opapp = Self::visit_opapp(opapp, ctx);
                let rw_typ = Self::visit_type(typ, ctx);
                Expr::OpApp(rw_opapp, rw_typ)
            },
            _ => panic!("Implementation error; Expected opapp expr."),
        }
    }
    fn visit_opapp(opapp: OpApp, ctx: &RefCell<C>) -> OpApp {
        let OpApp { op, operands } = opapp;
        let rw_op = Self::visit_op(op, ctx);
        let rw_operands = operands.into_iter().map(|operand| Self::visit_expr(operand, ctx)).collect::<Vec<_>>();
        let rw_opapp = OpApp { op: rw_op, operands: rw_operands };
        Self::rewrite_opapp(rw_opapp, ctx)
    }
    fn visit_op(op: Op, ctx: &RefCell<C>) -> Op {
        Self::rewrite_op(op, ctx)
    }
    fn visit_expr_funcapp(expr: Expr, ctx: &RefCell<C>) -> Expr {
        match expr {
            Expr::FuncApp(fapp, typ) => {
                let rw_fapp = Self::visit_fapp(fapp, ctx);
                let rw_typ = Self::visit_type(typ, ctx);
                Expr::FuncApp(rw_fapp, rw_typ)
            }
            _ => panic!("Implementation error; Funcapp expected."),
        }
    }
    fn visit_fapp(fapp: FuncApp, ctx: &RefCell<C>) -> FuncApp {
        let FuncApp { func_name, operands } = fapp;
        let rw_operands = operands.into_iter().map(|operand| Self::visit_expr(operand, ctx)).collect::<Vec<_>>();
        let rw_fapp = FuncApp { func_name: func_name.clone(), operands: rw_operands };
        Self::rewrite_funcapp(rw_fapp, ctx)
    }
    fn visit_stmt_funccall(stmt: Stmt, ctx: &RefCell<C>) -> Stmt {
        match stmt {
            Stmt::FuncCall(fc) => Stmt::FuncCall(Self::visit_funccall(fc, ctx)),
            _ => panic!("Implementation error; Expected FuncCall."),
        }
    }
    fn visit_funccall(fc: FuncCall, ctx: &RefCell<C>) -> FuncCall {
        let FuncCall { func_name, lhs, operands } = fc;
        let rw_lhs = lhs.into_iter().map(|e| Self::visit_expr(e, ctx)).collect::<Vec<_>>();
        let rw_operands = operands.into_iter().map(|e| Self::visit_expr(e, ctx)).collect::<Vec<_>>();
        let rw_funccall = FuncCall { func_name: func_name.clone(), lhs: rw_lhs, operands: rw_operands };
        Self::rewrite_funccall(rw_funccall, ctx)
    }
    fn visit_stmt_assign(stmt: Stmt, ctx: &RefCell<C>) -> Stmt {
        match stmt {
            Stmt::Assign(a) => Stmt::Assign(Self::visit_assign(a, ctx)),
            _ => panic!("Implementation error; Expected assign."),
        }
    }
    fn visit_assign(a: Assign, ctx: &RefCell<C>) -> Assign {
        let Assign { lhs, rhs } = a;
        let rw_lhs = lhs.into_iter().map(|e| Self::visit_expr(e, ctx)).collect::<Vec<_>>();
        let rw_rhs = rhs.into_iter().map(|e| Self::visit_expr(e, ctx)).collect::<Vec<_>>();
        let rw_assign = Assign { lhs: rw_lhs, rhs: rw_rhs };
        Self::rewrite_assign(rw_assign, ctx)
    }
    fn visit_stmt_ifthenelse(stmt: Stmt, ctx: &RefCell<C>) -> Stmt {
        match stmt {
            Stmt::IfThenElse(ite) => Stmt::IfThenElse(Self::visit_ite(ite, ctx)),
            _ => panic!("Implementation error; Expected ITE."),
        }
    }
    fn visit_ite(ite: IfThenElse, ctx: &RefCell<C>) -> IfThenElse {
        let IfThenElse { cond, then_stmt, else_stmt } = ite;
        let rw_cond = Self::visit_expr(cond, ctx);
        let rw_thn = Box::new(Self::visit_stmt(*then_stmt, ctx));
        let rw_els = else_stmt.map(|stmt| Box::new(Self::visit_stmt(*stmt, ctx)));
        let rw_ite = IfThenElse { cond: rw_cond, then_stmt: rw_thn , else_stmt: rw_els };
        Self::rewrite_ite(rw_ite, ctx)
    }
    fn visit_stmt_block(stmt: Stmt, ctx: &RefCell<C>) -> Stmt {
        let rw_stmt = match stmt {
            Stmt::Block(stmts) => {
                let rw_stmts = stmts.into_iter().map(|box_stmt| Box::new(Self::visit_stmt(*box_stmt, ctx))).collect::<Vec<_>>();
                Stmt::Block(rw_stmts)
            },
            _ => panic!("Implementation error; Expected block."),
        };
        Self::rewrite_stmt_block(rw_stmt, ctx)
    }
}

// =======================================================
/// ## AST Statements

#[derive(Clone)]
pub enum Stmt {
    Assume(Expr),
    FuncCall(FuncCall),
    Assign(Assign),
    IfThenElse(IfThenElse),
    Block(Vec<Box<Stmt>>),
    Comment(String),
}

impl Stmt {
    pub fn get_expect_block(&self) -> &Vec<Box<Stmt>> {
        match self {
            Stmt::Block(b) => b,
            _ => panic!("Not a block."),
        }
    }
    pub fn is_blk(&self) -> bool {
        if let Stmt::Block(_) = self {
            true
        } else {
            false
        }
    }
    pub fn func_call(func_name: String, lhs: Vec<Expr>, operands: Vec<Expr>) -> Self {
        Stmt::FuncCall(FuncCall {
            func_name,
            lhs,
            operands,
        })
    }
    pub fn if_then_else(cond: Expr, then_stmt: Box<Stmt>, else_stmt: Option<Box<Stmt>>) -> Self {
        Stmt::IfThenElse(IfThenElse {
            cond,
            then_stmt,
            else_stmt,
        })
    }
    pub fn assign(lhs: Vec<Expr>, rhs: Vec<Expr>) -> Self {
        Stmt::Assign(Assign { lhs, rhs })
    }
}

/// Function call statement
#[derive(Clone)]
pub struct FuncCall {
    pub func_name: String,
    pub lhs: Vec<Expr>,
    pub operands: Vec<Expr>,
}

/// Assign statement
#[derive(Clone)]
pub struct Assign {
    pub lhs: Vec<Expr>,
    pub rhs: Vec<Expr>,
}

/// If then else statement
#[derive(Clone)]
pub struct IfThenElse {
    pub cond: Expr,
    pub then_stmt: Box<Stmt>,
    pub else_stmt: Option<Box<Stmt>>,
}

// =======================================================
/// ## (Software) Procedure Model

#[derive(Clone)]
pub struct FuncModel {
    pub sig: FuncSig,
    pub body: Stmt,
    pub inline: bool,
}

/// Function Model for pre/post verification
impl FuncModel {
    pub fn new(
        name: &str,
        entry_addr: u64,
        arg_decls: Vec<Expr>,
        ret_decl: Option<Type>,
        requires: Option<Vec<sl_ast::Spec>>,
        ensures: Option<Vec<sl_ast::Spec>>,
        tracked: Option<Vec<sl_ast::Spec>>,
        mod_set: Option<HashSet<String>>,
        body: Stmt,
        inline: bool,
    ) -> Self {
        assert!(
            &body.is_blk(),
            "Body of {} should be a block.",
            name
        );
        let mod_set = mod_set.unwrap_or(HashSet::new());
        let requires = requires.unwrap_or(vec![]);
        let ensures = ensures.unwrap_or(vec![]);
        let tracked = tracked.unwrap_or(vec![]);
        FuncModel {
            sig: FuncSig::new(
                name, entry_addr, arg_decls, ret_decl, requires, ensures, tracked, mod_set,
            ),
            body: body,
            inline: inline,
        }
    }
}

/// Function signature
#[derive(Clone)]
pub struct FuncSig {
    pub name: String,
    pub entry_addr: u64,
    pub arg_decls: Vec<Expr>,
    pub ret_decl: Option<Type>,
    pub requires: Vec<sl_ast::Spec>,
    pub ensures: Vec<sl_ast::Spec>,
    pub tracked: Vec<sl_ast::Spec>,
    pub mod_set: HashSet<String>,
}

impl FuncSig {
    pub fn new(
        name: &str,
        entry_addr: u64,
        arg_decls: Vec<Expr>,
        ret_decl: Option<Type>,
        requires: Vec<sl_ast::Spec>,
        ensures: Vec<sl_ast::Spec>,
        tracked: Vec<sl_ast::Spec>,
        mod_set: HashSet<String>,
    ) -> Self {
        assert!(
            arg_decls.iter().all(|v| v.is_var()),
            "An argument of {} is not a variable.",
            name
        );
        FuncSig {
            name: String::from(name),
            entry_addr,
            arg_decls,
            ret_decl,
            requires,
            ensures,
            tracked,
            mod_set,
        }
    }
}

// =======================================================
/// ## Verification Model

#[derive(Clone)]
pub struct Model {
    pub name: String,
    pub vars: HashSet<Var>,
    pub func_models: Vec<FuncModel>,
}

impl Model {
    pub fn new(name: &str) -> Self {
        Model {
            name: String::from(name),
            vars: HashSet::new(),
            func_models: vec![],
        }
    }
    pub fn add_func_model(&mut self, fm: FuncModel) {
        if self
            .func_models
            .iter()
            .find(|fm_| fm_.sig.name == fm.sig.name)
            .is_none()
        {
            self.func_models.push(fm);
        }
    }
    pub fn add_func_models(&mut self, fms: Vec<FuncModel>) {
        for fm in fms {
            self.add_func_model(fm);
        }
    }
    pub fn add_var(&mut self, var: Var) {
        self.vars.insert(var);
    }
    pub fn add_vars(&mut self, vars: &HashSet<Var>) {
        for v in vars.iter() {
            self.add_var(v.clone());
        }
    }
}
