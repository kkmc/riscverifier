use std::{
    boxed::Box,
    collections::HashSet,
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    rc::Rc,
    cell::RefCell,
};

use topological_sort::TopologicalSort;

use asts::{spec_lang::sl_ast, veriv_ast::*};

use dwarf_ctx::dwarfreader::{DwarfCtx, DwarfTypeDefn};

use rv_model::system_model;

use utils::{constants, helpers};

use crate::{
    datastructures::cfg, disassembler::disassembler, disassembler::disassembler::Inst,
    ir_interface::IRInterface,
};

// ================================================================================
/// # VERI-V Translator

/// Instruction level translator from RISC-V to verification language IR
pub struct Translator<'t, I>
where
    I: IRInterface,
{
    // ====================================================================
    // Translator inputs
    /// Width of register in bits
    xlen: u64,
    /// Verification model
    model: Model,
    /// List of assembly instructions
    bbs: &'t HashMap<u64, Rc<cfg::BasicBlock<disassembler::AssemblyLine>>>,
    /// A set of the functions to ignore
    ignored_funcs: &'t HashSet<&'t str>,
    /// A list of functions to verify
    verify_funcs: &'t Vec<&'t str>,
    /// DWARF debugging information
    dwarf_ctx: &'t DwarfCtx,
    /// Map of specs from function name to a list of pre/post conditions
    specs_map: &'t HashMap<String, Vec<sl_ast::Spec>>,
    /// Flag indicating if the translator will ignore specs
    /// When true, all function pre and post conditions are ignored
    /// and functions are all inlined
    ignore_specs: bool,

    // ====================================================================
    // Translator context
    /// Map of function names / labels to entry addresses
    labels_to_addr: HashMap<String, u64>,
    /// Memoize map for generated functions at the given address
    cfg_memo: HashMap<u64, Rc<cfg::Cfg<disassembler::AssemblyLine>>>,
    /// Generated functions / labels by addresses
    generated: HashSet<u64>,
    /// Map of procedure name to thier modifies set
    mod_set_map: HashMap<String, HashSet<String>>,

    // =====================================================================
    // Phantom data
    _phantom_i: PhantomData<I>,
}

impl<'t, I> Translator<'t, I>
where
    I: IRInterface,
{
    /// Translator constructor
    pub fn new(
        xlen: u64,
        module_name: &'t str,
        bbs: &'t HashMap<u64, Rc<cfg::BasicBlock<disassembler::AssemblyLine>>>,
        ignored_funcs: &'t HashSet<&'t str>,
        verify_funcs: &'t Vec<&'t str>,
        dwarf_ctx: &'t DwarfCtx,
        specs_map: &'t HashMap<String, Vec<sl_ast::Spec>>,
        ignore_specs: bool,
    ) -> Self {
        // Initialize the VERI-V model
        let mut model = Model::new(module_name);
        model.add_vars(&system_model::sys_state_vars(xlen));

        // Create a translator
        Translator {
            // Inputs
            xlen: xlen,
            model,
            bbs: bbs,
            ignored_funcs: ignored_funcs,
            verify_funcs: verify_funcs,
            dwarf_ctx: dwarf_ctx,
            specs_map: specs_map,
            ignore_specs: ignore_specs,
            // Context
            labels_to_addr: Translator::<I>::create_label_to_addr_map(bbs),
            cfg_memo: HashMap::new(),
            generated: HashSet::new(),
            mod_set_map: HashMap::new(),
            _phantom_i: PhantomData,
        }
    }

    // =============================================================================
    // Translator context

    /// Clear translator context
    pub fn clear(&mut self) {
        self.model = Model::new(&self.model.name);
        self.generated = HashSet::new();
    }

    /// Returns a map of labels / function names to entry addresses
    pub fn create_label_to_addr_map(
        bbs: &HashMap<u64, Rc<cfg::BasicBlock<disassembler::AssemblyLine>>>,
    ) -> HashMap<String, u64> {
        let mut label_to_addr = HashMap::new();
        for (_, bb) in bbs {
            if bb.entry().is_label_entry() {
                let name = bb.entry().function_name().to_string();
                let addr = bb.entry().address();
                label_to_addr.insert(name, addr);
            }
        }
        label_to_addr
    }

    // =============================================================================
    // Helper functions

    /// Returns the string representation of the model
    pub fn print_model(&self) -> String {
        I::model_to_string(
            &self.xlen,
            &self.model,
            &self.dwarf_ctx,
            &self.ignored_funcs,
            &self.verify_funcs,
        )
    }

    /// Converts a dwarf type to IR type
    fn to_ir_type(dtd: &DwarfTypeDefn) -> Type {
        match dtd {
            DwarfTypeDefn::Primitive { bytes } => Type::Bv {
                w: bytes * constants::BYTE_SIZE,
            },
            DwarfTypeDefn::Array {
                in_typ,
                out_typ,
                bytes: _,
            } => Type::Array {
                in_typs: vec![Box::new(Self::to_ir_type(in_typ))],
                out_typ: Box::new(Self::to_ir_type(out_typ)),
            },
            DwarfTypeDefn::Struct { id, fields, bytes } => Type::Struct {
                id: id.clone(),
                fields: fields
                    .iter()
                    .map(|(id, struct_field)| {
                        (id.clone(), Box::new(Self::to_ir_type(&struct_field.typ)))
                    })
                    .collect::<BTreeMap<String, Box<Type>>>(),
                w: bytes * constants::BYTE_SIZE,
            },
            DwarfTypeDefn::Pointer {
                value_typ: _,
                bytes,
            } => Type::Bv {
                w: bytes * constants::BYTE_SIZE,
            },
        }
    }

    // =============================================================================
    // Translation functions

    /// Generates a stub function model
    pub fn gen_func_model_stub(&mut self, func_name: &str) {
        let arg_exprs = self
            .func_args(func_name)
            .iter()
            .map(|expr| {
                let var_name = expr.get_var_name();
                Expr::var(&var_name, system_model::bv_type(self.xlen))
            })
            .collect();
        let mod_set = self.mod_set_from_spec_map(func_name);
        let requires = if !self.ignore_specs {
            self.requires_from_spec_map(func_name)
        } else {
            None
        };
        let ensures = if !self.ignore_specs {
            self.ensures_from_spec_map(func_name)
        } else {
            None
        };
        let tracked = self.tracked_from_spec_map(func_name);
        let ret = None;
        let entry_addr = *self
            .func_entry_addr(func_name)
            .expect(&format!("Unable to find {}'s entry address.", func_name));
        let stub_fm = FuncModel::new(
            func_name,
            entry_addr,
            arg_exprs,
            ret,
            requires,
            ensures,
            tracked,
            mod_set,
            Stmt::Block(vec![]),
            false,
        );
        self.model.add_func_model(stub_fm);
    }

    /// Generates a model for the function at address "addr"
    pub fn gen_func_model(&mut self, func_name: &str) {
        // Skip the functions that have already been generated
        let func_entry = *self
            .func_entry_addr(func_name)
            .expect(&format!("Unable to find {}'s entry address.", func_name));
        if self.generated.get(&func_entry).is_some() {
            return;
        }

        // Mark the function as generated
        self.generated.insert(func_entry);

        // If the function is ignore, only generate a stub models
        if self.ignored_funcs.get(func_name).is_some() {
            self.gen_func_model_stub(func_name);
            return;
        }

        // Get the function cfg
        let func_cfg = self.get_func_cfg(func_entry);

        // ======= State variables ====================================
        // FIXME: Remove these later; these variables should be predefined in the rv_model library
        // Initialize global variables for the function block
        self.model.add_vars(&self.infer_vars(&func_cfg));

        // ====== Basic Block Function Models ==========================
        // Generate procedure model for each basic block
        let bb_fms = func_cfg
            .nodes()
            .iter()
            .map(|(addr, bb)| {
                // Generate basic blocks
                let bb_proc_name = self.bb_proc_name(*addr);
                let body = self.cfg_node_to_block(bb);
                
                // Passes to abstract memory
                let mut processed_body = ConstantPropagator::visit_stmt(body, &RefCell::new(&mut HashMap::new()));
                let mut abs_var_names = HashSet::new();
                processed_body = DataMemoryAbstractor::visit_stmt(processed_body, &RefCell::new(&mut abs_var_names));
                self.model.add_vars(&abs_var_names);
                
                let mod_set = self.infer_mod_set(&processed_body);
                FuncModel::new(
                    &bb_proc_name,
                    *addr,
                    vec![],
                    None,
                    None,
                    None,
                    None,
                    Some(mod_set),
                    processed_body,
                    true,
                )
            })
            .collect::<Vec<_>>();

        // ====== Modifies sets ============================================
        // Add all basic block mod sets to the model
        let bb_mod_sets = bb_fms
            .iter()
            .map(|fm| (fm.sig.name.clone(), fm.sig.mod_set.clone()))
            .collect::<Vec<(String, HashSet<String>)>>();
        for bb_mod_set in bb_mod_sets {
            self.mod_set_map.insert(bb_mod_set.0, bb_mod_set.1);
        }
        // Modifies set for the current function
        let mut mod_set = bb_fms
            .iter()
            .map(|bb_fm| bb_fm.sig.mod_set.clone())
            .flatten()
            .collect::<HashSet<String>>();
        // Add basic block function models to the model
        self.model.add_func_models(bb_fms);

        // ======== Recursively Generate Callees ===========================
        let callees = self.get_callee_addrs(func_name, &func_cfg);
        for (target, _) in &callees {
            if let Some(name) = self.get_func_at(target) {
                self.gen_func_model(&name[..]);
            }
        }
        // Add callee modifies set to this function's modifies set
        for (target, _) in &callees {
            if let Some(name) = self.get_func_at(target) {
                if !self.ignored_funcs.contains(&name[..]) {
                    continue;
                }
                if self.ignored_funcs.get(&name[..]).is_some() {
                    if let Some(ms) = self.mod_set_from_spec_map(func_name) {
                        mod_set = mod_set.union(&ms).cloned().collect();
                    } else {
                        // FIXME: Warn that we haven't provided a modifies set here?
                    }
                } else {
                    let callee_ms = self
                        .mod_set_map
                        .get(&name)
                        .expect(&format!("Unable to find modifies set for {}.", name));
                    mod_set = mod_set.union(callee_ms).cloned().collect();
                }
            }
        }

        // ================= Create function model ============================
        // Memo current mod set
        self.mod_set_map
            .insert(func_name.to_string(), mod_set.clone());
        // Get arguments of function
        let arg_exprs = self
            .func_args(func_name)
            .iter()
            .map(|expr| {
                let var_name = expr.get_var_name();
                Expr::var(&var_name, system_model::bv_type(self.xlen))
            })
            .collect();
        // Translate the specifications
        let requires = if !self.ignore_specs {
            self.requires_from_spec_map(func_name)
        } else {
            None
        };
        let ensures = if !self.ignore_specs {
            self.ensures_from_spec_map(func_name)
        } else {
            None
        };
        let tracked = self.tracked_from_spec_map(func_name);
        // Create the procedure body
        let body = self.cfg_to_symbolic_blk(&func_entry, &func_cfg);
        // Add the function to the verification model
        self.model.add_func_model(FuncModel::new(
            func_name,
            func_entry,
            arg_exprs,
            None,
            requires,
            ensures,
            tracked,
            Some(mod_set),
            body,
            self.ignore_specs,
        ));
    }

    /// Returns the inferred modifies set
    fn infer_mod_set(&self, stmt: &Stmt) -> HashSet<String> {
        let mut mod_set = HashSet::new();
        mod_set.insert(constants::PC_VAR.to_string());
        mod_set.insert(constants::RETURNED_FLAG.to_string());
        match stmt {
            Stmt::FuncCall(fc) => {
                // Add modifies set if it's a function call
                if let Some(fc_mod_set) = self.mod_set_map.get(&fc.func_name) {
                    mod_set = mod_set.union(&fc_mod_set).cloned().collect();
                }
                // Add the left hand assignments
                let lhs = fc
                    .lhs
                    .iter()
                    .map(|v| v.get_var_name())
                    .collect::<HashSet<_>>();
                mod_set = mod_set.union(&lhs).cloned().collect();
            }
            Stmt::Assign(a) => {
                let lhs_mod_set = a
                    .lhs
                    .iter()
                    .map(|e| match e {
                        // Either the LHS is a register, returned, pc, etc
                        Expr::Var(v, _) => v.name.clone(),
                        Expr::OpApp(opapp, _) => {
                            assert!(opapp.op == Op::ArrayIndex, "Assignment should be a register or memory index.");
                            // Or the LHS is memory (for stores)
                            match &opapp.operands[0] {
                                Expr::Var(v, _) => v.name.clone(),
                                _ => panic!("LHS of array index should be memory type but found {}.", &opapp.operands[0]),
                            }
                        }
                        _ => panic!("LHS of assign should be a register or memory."),
                    })
                    .collect::<HashSet<String>>();
                mod_set = mod_set.union(&lhs_mod_set).cloned().collect();
            }
            Stmt::IfThenElse(ite) => {
                let then_mod_set = self.infer_mod_set(&ite.then_stmt);
                mod_set = mod_set.union(&then_mod_set).cloned().collect();
                if let Some(else_stmt) = &ite.else_stmt {
                    let else_mod_set = self.infer_mod_set(else_stmt);
                    mod_set = mod_set.union(&else_mod_set).cloned().collect();
                }
            }
            Stmt::Block(blk) => {
                let blk_mod_sets = blk
                    .iter()
                    .map(|stmt| self.infer_mod_set(stmt))
                    .flatten()
                    .collect::<HashSet<String>>();
                mod_set = mod_set.union(&blk_mod_sets).cloned().collect();
            }
            _ => (),
        }
        mod_set
    }

    /// Returns a block statement for the CFG
    fn cfg_to_symbolic_blk(
        &self,
        func_entry_addr: &u64,
        cfg_rc: &Rc<cfg::Cfg<disassembler::AssemblyLine>>,
    ) -> Stmt {
        let mut stmts_vec = vec![];
        let sorted_entries = self.topo_sort(cfg_rc);
        for bb_entry in sorted_entries {
            let cfg_node = cfg_rc.nodes().get(&bb_entry).expect(&format!(
                "Unable to find CFG node with entry address {}.",
                bb_entry
            ));
            // Skip basic blocks that are entry addresses to functions (except for this function)
            // FIXME: This is not tested well. Check if trap_vector is properly generated.
            // Sometimes there are functions (e.g. trap_vector) that call basic blocks
            // from other functions. If this happens, we want to create a model that
            // contains basic blocks from both functions.
            if cfg_node.entry().is_label_entry() && bb_entry != *func_entry_addr {
                continue;
            }
            // Basic block call
            let bb_call_stmt =
                Box::new(Stmt::func_call(self.bb_proc_name(bb_entry), vec![], vec![]));
            let then_blk_stmt = Stmt::Block(vec![bb_call_stmt]);
            let guarded_call = Box::new(self.guarded_call(&bb_entry, then_blk_stmt));
            stmts_vec.push(guarded_call);
            // Function call
            // If the instruction is a jump and the target is
            // another function's entry address, then make a call to it.
            if cfg_node.exit().op() == constants::JAL {
                let target_addr = cfg_node
                    .exit()
                    .imm()
                    .expect("Invalid format: JAL is missing a target address.")
                    .get_imm_val() as u64;
                let target_cfg_node = cfg_rc.nodes().get(&target_addr).expect(&format!(
                    "Unable to find CFG node with entry address {}.",
                    bb_entry
                ));
                if target_cfg_node.entry().is_label_entry() {
                    // This is a function in the higher level code because the CFG node has an entry point
                    let f_name = self
                        .get_func_at(&target_addr)
                        .expect(&format!("Could not find function entry at {}.", bb_entry));
                    let f_args = self
                        .func_args(&f_name)
                        .iter()
                        .enumerate()
                        .map(|(i, arg_expr)| Expr::var(&format!("a{}", i), arg_expr.typ().clone()))
                        .collect::<Vec<_>>();
                    // TODO(kkmc): Ignore the return value. The current implementation does not
                    // use the return value and is only tested with functions that have single
                    // return values. hence lhss is left as an empty vector below.
                    let lhss = vec![];
                    // Construct the function call
                    let f_call_stmt = Box::new(Stmt::func_call(f_name, lhss, f_args));
                    let mut then_stmts = vec![];
                    // Add function call to then statement
                    then_stmts.push(f_call_stmt);
                    // Reset the returned variable for the caller
                    then_stmts.push(Box::new(Stmt::assign(
                        vec![Expr::var(
                            constants::RETURNED_FLAG,
                            system_model::bv_type(1),
                        )],
                        vec![Expr::bv_lit(0, 1)],
                    )));
                    let then_blk_stmt = Stmt::Block(then_stmts);
                    let guarded_call = Box::new(self.guarded_call(&target_addr, then_blk_stmt));
                    stmts_vec.push(guarded_call)
                }
            }
        }
        stmts_vec.push(Box::new(Stmt::assign(
            vec![Expr::var(
                constants::RETURNED_FLAG,
                system_model::bv_type(1),
            )],
            vec![Expr::bv_lit(1, 1)],
        )));
        Stmt::Block(stmts_vec)
    }

    /// Returns a guarded block statement
    /// Guards are pc == target and returned == false
    fn guarded_call(&self, entry: &u64, blk: Stmt) -> Stmt {
        let if_pc_guard = Expr::op_app(
            Op::Comp(CompOp::Equality),
            vec![
                Expr::Var(
                    system_model::pc_var(self.xlen),
                    system_model::bv_type(self.xlen),
                ),
                Expr::bv_lit(*entry, self.xlen),
            ],
        );
        let if_returned_guard = Expr::op_app(
            Op::Comp(CompOp::Equality),
            vec![
                Expr::var(constants::RETURNED_FLAG, system_model::bv_type(1)),
                Expr::bv_lit(0, 1),
            ],
        );
        let if_guard = Expr::op_app(Op::Bool(BoolOp::Conj), vec![if_pc_guard, if_returned_guard]);
        let then_blk_stmt = Box::new(blk);
        // Return the guarded call
        Stmt::if_then_else(if_guard, then_blk_stmt, None)
    }

    /// Returns a topological sort of the cfg as an array of entry addresses
    fn topo_sort(&self, cfg_rc: &Rc<cfg::Cfg<disassembler::AssemblyLine>>) -> Vec<u64> {
        let mut ts = TopologicalSort::<u64>::new();
        // Initialize the first entry address of the CFG
        ts.insert(*cfg_rc.entry_addr());
        // Closure that determines the subgraphs to ignore by entry address
        let ignore = |addr| {
            self.get_func_at(&addr).is_some()
                && self
                    .ignored_funcs
                    .contains(&self.get_func_at(&addr).unwrap()[..])
        };
        // Recursively update ts to contain all the dependencies between basic blocks in the CFG
        self.compute_deps(
            &ignore,
            cfg_rc,
            cfg_rc.entry_addr(),
            &mut ts,
            &mut HashSet::new(),
        );
        // Convert to an array of sorted addresses by dependency
        let mut sorted = vec![];
        loop {
            let mut v = ts.pop_all();
            if v.is_empty() {
                if ts.len() != 0 {
                    // If ts.pop_all() is empty and len() != 0, there is a cycle
                    let cycle = cfg_rc
                        .find_cycle(
                            &ignore,
                            cfg_rc.entry_addr(),
                            &mut HashSet::new(),
                            &mut false,
                        )
                        .expect("Should have found a cycle.");
                    panic!(
                        "There is a cycle in the cfg of {:?}: {:?}.",
                        self.get_func_at(&cfg_rc.entry_addr()),
                        cycle
                            .iter()
                            .rev()
                            .map(|v| format!("{:#x?}", v))
                            .collect::<Vec<String>>()
                    )
                } else {
                    // Otherwise it's the end of the topological sort
                    break;
                }
            }
            v.sort();
            sorted.extend(v);
        }
        sorted
    }

    /// Recursively computes the dependency graph given the entry address
    /// However, it ignores all subgraphs rooted at cfg nodes with an entry address
    /// in which the closure "ignore" returns true for.
    fn compute_deps(
        &self,
        ignore: &dyn Fn(u64) -> bool,
        cfg_rc: &Rc<cfg::Cfg<disassembler::AssemblyLine>>,
        curr: &u64,
        ts: &mut TopologicalSort<u64>,
        processed: &mut HashSet<u64>,
    ) {
        if processed.contains(curr) {
            return;
        }
        processed.insert(*curr);
        if let Some(cfg_node) = cfg_rc.nodes().get(curr) {
            let entry = cfg_node.entry().address();
            if ignore(entry) {
                return;
            }
            for target in cfg_node.exit().successors() {
                ts.add_dependency(entry, target);
                // If the entry address is to a function entry,
                // then there is no need to recursively compute
                // the dependents of the callee because
                if cfg_rc
                    .nodes()
                    .get(&target)
                    .expect("Unable to find target basic block.")
                    .entry()
                    .is_label_entry()
                {
                    continue;
                }
                // Otherwise, recursively compute the dependencies of the target
                self.compute_deps(ignore, cfg_rc, &target, ts, processed);
            }
        } else {
            panic!("Unable to find basic block at {}", curr);
        }
    }

    /// Returns the function defined at the address "addr"
    fn get_func_at(&self, addr: &u64) -> Option<String> {
        let entry_blk = self
            .bbs
            .get(addr)
            .expect(&format!("Could not find basic block at {}.", addr))
            .entry();
        if entry_blk.is_label_entry() {
            Some(entry_blk.function_name().to_string())
        } else {
            None
        }
    }

    /// Returns a list of callee addresses and the lines they were called at
    ///
    /// # EXAMPLE
    /// 0000000080004444 <osm_pmp_set+0xc> jal  zero,0000000080004d58 <pmp_set>
    /// The line above would be added as (0000000080004d58, 0000000080004444)
    fn get_callee_addrs(
        &self,
        func_name: &str,
        cfg_rc: &Rc<cfg::Cfg<disassembler::AssemblyLine>>,
    ) -> Vec<(u64, u64)> {
        let mut callee_addrs = vec![];
        for (_, cfg_node) in cfg_rc.nodes() {
            for al in cfg_node.into_iter() {
                if al.function_name() != func_name {
                    continue;
                }
                if al.op() == constants::JAL {
                    callee_addrs.push((al.imm().unwrap().get_imm_val() as u64, al.address()));
                }
            }
        }
        callee_addrs
    }

    /// Returns the function name for basic blocks
    fn bb_proc_name(&self, addr: u64) -> String {
        format!("bb_{:#x?}", addr)
    }

    /// Returns a block statement given representing the basic block
    fn cfg_node_to_block(&self, bb: &Rc<cfg::CfgNode<disassembler::AssemblyLine>>) -> Stmt {
        let mut stmt_vec = vec![];
        for al in bb.into_iter() {
            // stmt_vec.push(Box::new(self.al_to_ir(&al)));
            stmt_vec.push(Box::new(self.al_to_ir_stmt(&al)));
        }
        Stmt::Block(stmt_vec)
    }

    /// Returns the instruction / assembly line (al) in the VERI-V IR
    fn al_to_ir_stmt(&self, al: &Rc<disassembler::AssemblyLine>) -> Stmt {
        // Destination registers
        let mut dsts = vec![];
        let mut regs: [Option<&disassembler::InstOperand>; 2] = [al.rd(), al.csr()];
        for reg_op in regs.iter_mut() {
            if let Some(reg) = reg_op {
                dsts.push(Expr::var(
                    &reg.get_reg_name()[..],
                    system_model::bv_type(self.xlen),
                ));
                assert!(!reg.has_offset());
            }
        }
        // Source registers
        let mut srcs = vec![];
        let mut regs: [Option<&disassembler::InstOperand>; 3] = [al.rs1(), al.rs2(), al.csr()];
        for reg_op in regs.iter_mut() {
            if let Some(reg) = reg_op {
                let reg_name = &reg.get_reg_name()[..];
                match reg_name {
                    // Replace the zero register with a 0 constant
                    // the zero register is used as a placeholder for
                    // writing to in the verification models
                    "zero" => srcs.push(Expr::bv_lit(0, self.xlen)),
                    _ => srcs.push(Expr::var(reg_name, system_model::bv_type(self.xlen))),
                }
                if reg.has_offset() {
                    srcs.push(Expr::bv_lit(reg.get_reg_offset() as u64, self.xlen));
                }
            }
        }
        if let Some(operand) = al.imm() {
            srcs.push(Expr::bv_lit(operand.get_imm_val() as u64, self.xlen));
        }
        match al.op() {
            "add" => {
                system_model::add_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "sub" => {
                system_model::sub_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "mul" => {
                system_model::mul_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "sll" => {
                system_model::sll_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "slt" => {
                system_model::slt_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "sltu" => system_model::sltu_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "xor" => {
                system_model::xor_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "srl" => {
                system_model::srl_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "sra" => {
                system_model::sra_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "or" => {
                system_model::or_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "and" => {
                system_model::and_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "addw" => system_model::addw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "subw" => system_model::subw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "sllw" => system_model::sllw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "srlw" => system_model::srlw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "sraw" => system_model::sraw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "jalr" => system_model::jalr_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "lb" => {
                system_model::lb_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "lh" => {
                system_model::lh_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "lw" => {
                system_model::lw_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "lbu" => {
                system_model::lbu_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "lhu" => {
                system_model::lhu_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "addi" => system_model::addi_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "slti" => system_model::slti_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "sltiu" => system_model::sltiu_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "xori" => system_model::xori_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "ori" => {
                system_model::ori_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "andi" => system_model::andi_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "slli" => system_model::slli_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "srli" => system_model::srli_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "srai" => system_model::srai_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "lwu" => {
                system_model::lwu_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "ld" => {
                system_model::ld_inst(dsts[0].clone(), srcs[0].clone(), srcs[1].clone(), self.xlen)
            }
            "addiw" => system_model::addiw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "slliw" => system_model::slliw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "srliw" => system_model::srliw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "sraiw" => system_model::sraiw_inst(
                dsts[0].clone(),
                srcs[0].clone(),
                srcs[1].clone(),
                self.xlen,
            ),
            "sb" => {
                system_model::sb_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "sh" => {
                system_model::sh_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "sw" => {
                system_model::sw_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "sd" => {
                system_model::sd_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "beq" => {
                system_model::beq_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "bne" => {
                system_model::bne_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "blt" => {
                system_model::blt_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "bge" => {
                system_model::bge_inst(srcs[0].clone(), srcs[1].clone(), srcs[2].clone(), self.xlen)
            }
            "bltu" => system_model::bltu_inst(
                srcs[0].clone(),
                srcs[1].clone(),
                srcs[2].clone(),
                self.xlen,
            ),
            "bgeu" => system_model::bgeu_inst(
                srcs[0].clone(),
                srcs[1].clone(),
                srcs[2].clone(),
                self.xlen,
            ),
            "lui" => system_model::lui_inst(dsts[0].clone(), srcs[0].clone(), self.xlen),
            "auipc" => system_model::auipc_inst(dsts[0].clone(), srcs[0].clone(), self.xlen),
            "jal" => system_model::jal_inst(dsts[0].clone(), srcs[0].clone(), self.xlen),
            _ => system_model::unimplemented_inst(al.op(), self.xlen),
        }
    }

    /// Constructs and returns a pointer to a Cfg with entry address addr
    fn get_func_cfg(&mut self, addr: u64) -> Rc<cfg::Cfg<disassembler::AssemblyLine>> {
        if let Some(cfg_rc) = self.cfg_memo.get(&addr) {
            return Rc::clone(cfg_rc);
        }
        let entry_bb = self
            .bbs
            .get(&addr)
            .expect(&format!("Unable to basic block at {}.", addr));
        assert!(
            &entry_bb.entry().is_label_entry(),
            "{} is not an entry address to a function.", addr
        );
        let cfg = Rc::new(cfg::Cfg::new(addr, &self.bbs));
        self.cfg_memo.insert(addr, Rc::clone(&cfg));
        cfg
    }

    /// Infer register variables from cfg.
    /// FIXME: Remove this function, eventually the system model should be entirely predefined.
    fn infer_vars(&self, cfg_rc: &Rc<cfg::Cfg<disassembler::AssemblyLine>>) -> HashSet<Var> {
        let mut var_names = vec![];
        for (_, cfg_node) in cfg_rc.nodes() {
            for al in cfg_node.into_iter() {
                let mut regs: [Option<&disassembler::InstOperand>; 4] =
                    [al.rd(), al.rs1(), al.rs2(), al.csr()];
                for reg_op in regs.iter_mut() {
                    if let Some(reg) = reg_op {
                        var_names.push(reg.to_string());
                    }
                }
            }
        }
        var_names
            .iter()
            .cloned()
            .map(|vid| Var {
                name: vid,
                typ: system_model::bv_type(self.xlen),
            })
            .collect::<HashSet<Var>>()
    }

    /// Returns the arguments of a function from the DWARF context
    fn func_args(&self, func_name: &str) -> Vec<Expr> {
        self.dwarf_ctx
            .func_sig(func_name)
            .ok()
            .and_then(|fs| {
                Some(
                    fs.args
                        .iter()
                        .map(|x| Expr::var(&x.name[..], Self::to_ir_type(&x.typ_defn)))
                        .collect::<Vec<Expr>>(),
                )
            })
            .map_or(vec![], |v| v)
    }

    /// Returns the entry address of the function named `func_name`
    fn func_entry_addr(&self, func_name: &str) -> Option<&u64> {
        self.labels_to_addr.get(func_name)
    }

    // =============================================================================
    // Specification retrieval helper functions

    /// Returns a vector of specifications from function named `func_name` if
    /// it exists in the specification map `spec_map`
    /// It filters out the specifications according to sfilter
    fn filter_from_spec_map(
        &self,
        func_name: &str,
        sfilter: fn(&sl_ast::Spec) -> bool,
    ) -> Option<Vec<sl_ast::Spec>> {
        let specs = match self.specs_map.get(func_name) {
            Some(spec_vec) => spec_vec
                .iter()
                .filter(|spec| sfilter(*spec))
                .cloned()
                .collect::<Vec<sl_ast::Spec>>(),
            None => return None,
        };
        Some(specs)
    }

    /// Returns a single hash set containing all variables in the modifies set(s)
    fn mod_set_from_spec_map(&mut self, func_name: &str) -> Option<HashSet<String>> {
        let sfilter = |s: &sl_ast::Spec| match s {
            sl_ast::Spec::Modifies(..) => true,
            _ => false,
        };
        let specs = self.filter_from_spec_map(func_name, sfilter);
        // Combine the modifies set if there are any (we only need one)
        match specs {
            Some(specs) => {
                let combined_modset = specs
                    .iter()
                    .map(|spec| match &*spec {
                        sl_ast::Spec::Modifies(hs) => hs,
                        _ => panic!("Should have filtered non modifies specifications."),
                    })
                    .flatten()
                    .cloned()
                    .collect::<HashSet<String>>();
                Some(combined_modset)
            }
            None => None,
        }
    }

    /// Returns a vector of require statements for function `func_name`
    fn requires_from_spec_map(&self, func_name: &str) -> Option<Vec<sl_ast::Spec>> {
        let sfilter = |s: &sl_ast::Spec| match s {
            sl_ast::Spec::Requires(..) => true,
            _ => false,
        };
        self.filter_from_spec_map(func_name, sfilter)
    }

    /// Returns a vector of ensure statements for function `func_name`
    fn ensures_from_spec_map(&self, func_name: &str) -> Option<Vec<sl_ast::Spec>> {
        let sfilter = |s: &sl_ast::Spec| match s {
            sl_ast::Spec::Ensures(..) => true,
            _ => false,
        };
        self.filter_from_spec_map(func_name, sfilter)
    }

    /// Returns a vector of track statements for function `func_name`
    fn tracked_from_spec_map(&self, func_name: &str) -> Option<Vec<sl_ast::Spec>> {
        let sfilter = |s: &sl_ast::Spec| match s {
            sl_ast::Spec::Track(..) => true,
            _ => false,
        };
        self.filter_from_spec_map(func_name, sfilter)
    }
}

// ================================================================================
/// # VERI-V AST Rewriters

/// Constant propagation rewriter
struct ConstantPropagator;
impl ConstantPropagator {
    /// Tries to evaluate the value of expression
    fn constant_fold(expr: Expr) -> Expr {
        if let Expr::OpApp(opapp, typ) = expr {
            let OpApp { op, operands } = opapp;
            let rw_operands = operands.into_iter().map(|operand| Self::constant_fold(operand)).collect::<Vec<_>>();
            let oper1 = rw_operands.get(0).unwrap();
            let oper2_opt = rw_operands.get(1); // second operand only appears in some operations
            // If the operands exist, then they should be literals
            if !(oper1.is_lit() && oper2_opt.map_or(true, |oper| oper.is_lit())) {
                return Expr::OpApp(OpApp { op, operands: rw_operands }, typ);
            }
            let oper1_val: u64 = oper1.get_lit_value().unwrap();
            let oper2_val_opt: Option<u64> = oper2_opt.map(|oper| oper.get_lit_value().unwrap());
            match op {
                Op::Comp(cop) => {
                    let oper2_val = oper2_val_opt.expect(&format!("Second argument missing for constant folding {:?}.", cop));
                    match cop {
                        CompOp::Equality => Expr::bool_lit(oper1_val == oper2_val),
                        CompOp::Inequality => Expr::bool_lit(oper1_val != oper2_val),
                        // TODO: Check if this cast does signed comparison
                        CompOp::Lt => Expr::bool_lit((oper1_val as i64) < (oper2_val as i64)),  // <
                        CompOp::Le => Expr::bool_lit(oper1_val as i64 <= oper2_val as i64),  // <=
                        CompOp::Gt => Expr::bool_lit(oper1_val as i64 > oper2_val as i64),  // >
                        CompOp::Ge => Expr::bool_lit(oper1_val as i64 >= oper2_val as i64),  // >=
                        CompOp::Ltu => Expr::bool_lit(oper1_val < oper2_val), // <_u (unsigned)
                        CompOp::Leu => Expr::bool_lit(oper1_val <= oper2_val), // <=_u
                        CompOp::Gtu => Expr::bool_lit(oper1_val > oper2_val), // >_u
                        CompOp::Geu => Expr::bool_lit(oper1_val >= oper2_val), // >=_u
                    }
                },
                Op::Bv(bvop) => {
                    match bvop {
                        BVOp::Add => Expr::bv_lit(oper1_val + oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        BVOp::Sub => Expr::bv_lit(oper1_val - oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        BVOp::Mul => Expr::bv_lit(oper1_val * oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        BVOp::And => Expr::bv_lit(oper1_val & oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        BVOp::Or => Expr::bv_lit(oper1_val | oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        BVOp::Xor => Expr::bv_lit(oper1_val ^ oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        BVOp::SignExt => Expr::bv_lit(oper1_val, oper1.get_expect_bv_width() + oper2_val_opt.unwrap()), // TODO: Double check; value should be signed 64
                        BVOp::ZeroExt => Expr::bv_lit(oper1_val, oper1.get_expect_bv_width() + oper2_val_opt.unwrap()),
                        BVOp::LeftShift => Expr::bv_lit(oper1_val << oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        BVOp::RightShift => Expr::bv_lit(((oper1_val as i64) >> oper2_val_opt.unwrap()) as u64, oper1.get_expect_bv_width()),
                        BVOp::ARightShift => Expr::bv_lit(oper1_val >> oper2_val_opt.unwrap(), oper1.get_expect_bv_width()),
                        // TODO: Implement concat, this just returns the original expression
                        BVOp::Concat => Expr::OpApp(OpApp { op: Op::Bv(bvop), operands: rw_operands }, typ),
                        BVOp::Slice { l, r } => Expr::bv_lit(oper1_val & helpers::mask(l, r), l-r+1),
                    }
                },
                Op::Bool(bop) => {
                    match bop {
                        BoolOp::Conj => Expr::bool_lit(oper1_val + oper2_val_opt.unwrap() == 2),
                        BoolOp::Disj => Expr::bool_lit(oper1_val + oper2_val_opt.unwrap() > 0),
                        BoolOp::Iff => Expr::bool_lit(oper1_val == oper2_val_opt.unwrap()),
                        BoolOp::Impl => Expr::bool_lit(oper1_val <= oper2_val_opt.unwrap()),
                        BoolOp::Neg => Expr::bool_lit(if oper1_val == 1 { false } else { true }),
                    }
                },
                _ => Expr::OpApp(OpApp { op, operands: rw_operands } , typ),
            }
        } else {
            expr
        }
    }

    /// Replaces all variables with constants
    fn constified_expr(expr: Expr, ctx: &RefCell<&mut HashMap<String, u64>>) -> Expr {
         match expr {
            Expr::Literal(lit, typ) => Expr::Literal(lit, typ),
            Expr::Var(var, vtyp) => {
                let Var { name, typ } = &var;
                match ctx.borrow().get(name) {
                    Some(val) => {
                        match typ {
                            Type::Bv { w } => Expr::bv_lit(*val, *w),
                            Type::Bool => Expr::bool_lit(if *val == 1 { true } else { false }),
                            Type::Int => Expr::int_lit(*val),
                            _ => Expr::Var(var, vtyp),
                        }
                    }
                    None => Expr::Var(var, vtyp)
                }
            }
            Expr::OpApp(opapp, _) => {
                let OpApp { op, operands } = opapp;
                let rw_operands = operands.into_iter().map(|expr| Self::constified_expr(expr, ctx)).collect::<Vec<_>>();
                Expr::op_app(op, rw_operands)
            }
            Expr::FuncApp(fapp, typ) => {
                let FuncApp { func_name, operands } = fapp;
                let rw_operands = operands.into_iter().map(|expr| Self::constified_expr(expr, ctx)).collect::<Vec<_>>();
                Expr::func_app(func_name, rw_operands, typ)
            }
         }
    }

    /// Replaces variables with contants (via constant propagation map) and returns the constant folded expression
    fn try_make_constant(expr: Expr, ctx: &RefCell<&mut HashMap<String, u64>>) -> Expr {
        let constified_expr = Self::constified_expr(expr, ctx);
        Self::constant_fold(constified_expr)
    }

    /// Updates the constant map
    fn constant_propagate(id: String, expr: Expr, ctx: &RefCell<&mut HashMap<String, u64>>) -> Expr {
        let folded_expr = Self::try_make_constant(expr, ctx);
        match folded_expr {
            Expr::Literal(_, _) => {
                let mut context = ctx.borrow_mut();
                context.insert(id, folded_expr.get_lit_value().unwrap());
            },
            _ => (),
        };
        folded_expr
    }
}

impl ASTRewriter<&mut HashMap<String, u64>> for ConstantPropagator {
    // Ignore the ITEs (there are only one level ITEs, don't constant propagate here)
    // and conservatively clear the map
    fn visit_stmt_ifthenelse(stmt: Stmt, ctx: &RefCell<&mut HashMap<String, u64>>) -> Stmt {
        match &stmt {
            Stmt::IfThenElse(_) => {
                ctx.borrow_mut().clear();
                stmt
            },
            _ => panic!("Implementation error; Expected ITE."),
        }
    }

    // Propagate all sequential assignments
    fn rewrite_assign(a: Assign, ctx: &RefCell<&mut HashMap<String, u64>>) -> Assign {
        let Assign { lhs, rhs } = a;
        let mut rw_lhss: Vec<Expr> = vec![];
        let mut rw_rhss: Vec<Expr> = vec![];
        for (l, r) in lhs.into_iter().zip(rhs) {
            let (rw_lhs, rw_rhs) = match &l {
                // when the LHS is just a variable, constant propagate the RHS to the LHS variable
                Expr::Var(var, _) => {
                    let rw_r = Self::constant_propagate(var.name.to_string(), r, ctx);
                    if !rw_r.is_lit() {
                        ctx.borrow_mut().remove(&var.name);
                    }
                    (l, rw_r)
                }
                // when the LHS is an array access, fold both the RHS and LHS (no constant propagation)
                Expr::OpApp(opapp, _) => {
                    let OpApp { op, operands: _ } = &opapp;
                    match op {
                        // check it's an array index
                        Op::ArrayIndex => {
                            match opapp.get_array_index() {
                                Some(index) => {
                                    let folded_index = Self::try_make_constant(index.clone(), ctx);
                                    let array = opapp.get_array_expr().expect("Left hand side should be an array.");
                                    let rw_l = Expr::op_app(Op::ArrayIndex, vec![array.clone(), folded_index]);
                                    let rw_r = Self::try_make_constant(r, ctx);
                                    (rw_l, rw_r)
                                }
                                None => panic!("Left hand side of an assignment should be an array."),
                            }
                        }
                        _ => (l, r)
                    } 
                }
                _ => (l, r)
            };
            rw_lhss.push(rw_lhs);
            rw_rhss.push(rw_rhs);
        }
        Assign { lhs: rw_lhss, rhs: rw_rhss }
    }
}

/// Intended for abstracting memory accesses whose addresses are constant, we abstract them as separate variables
/// 
/// Procedure:
///     1. Constant propagation for all variables
///     2. If a memory access has a constant address AND it is one of the global variable addresses,
///        then replace the memory access with a fresh variable corresponding to that global. Any
///        stores and load to that address will use this fresh variable.
///
/// NOTE: This assumes that all memory address computations are within thier own basic block
struct DataMemoryAbstractor;
impl ASTRewriter<&mut HashSet<Var>> for DataMemoryAbstractor {
    /// Rewrite all accesses to a contant address to the corressponding abstracted variable
    fn rewrite_expr(expr: Expr, ctx: &RefCell<&mut HashSet<Var>>) -> Expr {
        match &expr.get_array_index() {
            Some(index) => {
                // If the array access is a literal, then it should be a data variable
                if index.is_lit() {
                    // add variable to set
                    let w = match &expr.get_array_expr().expect("Expected array variable.").get_var_name()[..] {
                        constants::MEM_VAR_B => constants::BYTE_SIZE,
                        constants::MEM_VAR_H => constants::BYTE_SIZE*2,
                        constants::MEM_VAR_W => constants::BYTE_SIZE*4,
                        constants::MEM_VAR_D => constants::BYTE_SIZE*8,
                        _ => panic!("Expected byte, half, word, or double memory variable."),
                    };
                    let abs_var_name = helpers::abs_access_name(&index.get_lit_value().unwrap());
                    ctx.borrow_mut().insert(Var { name: abs_var_name.clone(), typ: Type::Bv { w }});
                    Expr::var(&abs_var_name, expr.typ().clone())
                } else {
                    expr
                }
            }
            None => expr
        }
    }
}

