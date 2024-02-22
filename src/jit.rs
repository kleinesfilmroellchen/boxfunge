//! # Befunge Just-in-time compiler
//!
//! The JIT compiler translates Befunge basic blocks to optimized byte code. Basic blocks consist of three main parts:
//! - A list of cells which are covered by this basic block. This is used to invalidate and recompile the basic block on playfield modifications as needed. Additionally, basic blocks are identified by their first PC, i.e. their entry point. Note that PC is not just position, but also direction, so two basic blocks may start at the same position as long as they go in different directions. This is of course impossible for basic blocks that start on a PC redirection command.
//! - A list of operations (the "byte code") that are executed in this basic block.
//! - A control-flow decision that happens after the basic block has executed. This may be unconditional (jump to this specific other basic block, or end the program) or conditional (jump to one of a selection of basic blocks depending on randomness or a condition value).
//!
//! Basic blocks are translated only as needed, so code that is never executed does not cause extra overhead. Also, code that is invalid does not cause an error. However, invalid code that *will* be executed as part of the currently encountered new basic block will immediately cause an error.
//!
//! Basic blocks merging together without conditionals may result in the same source code being compiled into multiple basic blocks. This is not a correctness problem, since the basic blocks will use the same final conditionals and converge as expected. Additionally, it may be an advantage, if the different initial paths yield different possible optimizations.
//!
//! During compilation, basic blocks are terminated in three situations, corresponding to the three control-flow cases:
//! - If a basic block enters the same PC of the start of another basic block (even itself), the basic block is terminated and jumps to that other basic block unconditionally.
//! - If a basic block encounters a conditional control flow command, it is compiled into its corresponding basic block control-flow decision and the basic block is terminated with it.
//! - If a basic block encounters an end program command, an end program terminator is used for the basic block.
//!
//! Loops are handled in the following way: If, during analysis, a basic block's path happens to reach a PC that is already in the basic block itself, it is terminated and jumps to that PC instead. This means that when the PC reaches this position later on and another basic block is compiled, it will eventually find its own entry point again and finish compiling; the basic block's control flow decision then indicates to unconditionally jump to itself, which represents the loop accurately. Note that sometimes, by chance, a looping section of code can immediately be detected as such if it hits the entry point of the PC. However, if code loops back into the middle of itself, the more advanced analysis is still necessary. Due to self-modification, basic block entry points are not predictable either, so those cases are covered by this as well. All of this means that during compilation, we keep track of every single PC that is encountered within the basic block, but we don't need this information after compilation.
//!
//! When a self-modification occurs, all basic blocks on the path of the self-modification are discarded. Execution continues from the next cell as given in the SetValue byte code op if the basic block invalidated itself. The latter is less efficient than it could be, since we could continue executing the basic block if the modification happened only for byte code ops that have already been executed. However, this would require keeping track of source cells for each byte code op, which seems like a significant complication in terms of memory management (one byte code op may refer to many source cells after optimization). It is not clear if this actually provides a practical benefit, since a lot of code is not immediately self-modifying, rather modifying some other (often distant) basic block.

use std::collections::HashMap;
use std::io;
use std::io::Read;
use std::io::Write;
use std::rc::Rc;

use crate::Error;
use crate::Executer;
use crate::Grid;
use crate::Int;
use crate::Interpreter;
use crate::Position;
use crate::PC;

/// JIT byte code operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    /// Push constant value to stack.
    PushConstant(Int),
    /// Duplicate top of stack.
    Duplicate,
    /// Swap top two stack values.
    Swap,
    /// Drop top of stack.
    Drop,
    /// Operations that map two stack values to one new value.
    Binary(BinaryOperation),
    /// Operations that map one stack value to one new value.
    /// The only current instance of this is the negation operation.
    Negate,
    /// Input a value.
    Input(IOMode),
    /// Output a value.
    Output(IOMode),
    /// Read from playfield.
    GetValue,
    /// Write to playfield. This is the most costly and complex operation, as it usually involves basic block invalidation.
    SetValue {
        /// PC after the SetValue operation. This is needed in case the current basic block invalidates itself, so that no state information is lost.
        pc_after: PC,
    },
}

/// The type of the I/O operation to perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IOMode {
    /// Input and output raw ASCII bytes.
    Ascii,
    /// Input and output decimal numbers, separated by whitespace or EOF.
    Decimal,
}

/// Operations that map two stack values to one new value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinaryOperation {
    Add,
    Subtract,
    Divide,
    Multiply,
    Remainder,
    Greater,
}

impl BinaryOperation {
    pub fn call(self, a: Int, b: Int) -> Int {
        match self {
            Self::Add => a.wrapping_add(b),
            Self::Subtract => a.wrapping_sub(b),
            Self::Divide => a.wrapping_div(b),
            Self::Multiply => a.wrapping_mul(b),
            Self::Remainder => a.wrapping_rem(b),
            Self::Greater => {
                if a > b {
                    1
                } else {
                    0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlFlowDecision {
    Jump(PC),
    Branch { true_target: PC, false_target: PC },
    EndProgram,
    Random { choices: [PC; 4] },
}

#[derive(Clone)]
struct BasicBlock {
    entry_point: PC,
    cells: Vec<Position>,
    bytecode: Vec<Operation>,
    next_bytecode_op: usize,
    cf_decision: ControlFlowDecision,
}

impl std::hash::Hash for BasicBlock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.entry_point.hash(state);
    }
}

impl BasicBlock {
    fn execute(&self, jit: &mut JustInTimeCompiler<'_>) -> Result<ControlFlowDecision, Error> {
        todo!()
    }
}

pub struct JustInTimeCompiler<'rw> {
    basic_blocks: HashMap<PC, Rc<BasicBlock>>,
    program_grid: Grid,
    stack: Vec<Int>,
    program_counter: PC,
    // I/O
    input: Box<dyn Read + 'rw>,
    output: Box<dyn Write + 'rw>,
    rng: random::Default,
}

impl<'rw> JustInTimeCompiler<'rw> {
    pub fn new(grid: &str) -> Result<Self, Error> {
        let input = Box::new(io::stdin());
        let output = Box::new(io::stdout());
        Self::new_with_io(grid, input, output)
    }

    pub fn new_with_io(
        grid: &str,
        input: Box<dyn Read + 'rw>,
        output: Box<dyn Write + 'rw>,
    ) -> Result<Self, Error> {
        let start = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let parsed_grid = Interpreter::parse_grid(grid)?;
        Ok(Self {
            basic_blocks: HashMap::new(),
            stack: Vec::new(),
            program_grid: parsed_grid,
            program_counter: PC::default(),
            input,
            output,
            rng: random::Default::new([start.to_bits(), start.to_bits()]),
        })
    }

    fn ensure_basic_block(&mut self) -> Result<Rc<BasicBlock>, Error> {
        todo!("compile")
    }
}

impl<'rw> Executer for JustInTimeCompiler<'rw> {
    fn run_forever(&mut self) -> Result<(), Error> {
        loop {
            let basic_block = self.ensure_basic_block()?;
            let result = basic_block.execute(self);
            match result {
                Ok(cf_decision) => {
                    todo!("execute the control flow");
                }
                Err(Error::ProgramEnd) => return Ok(()),
                Err(why) => return Err(why),
            }
        }
    }

    fn steps(&self) -> usize {
        1
    }

    fn position(&self) -> Position {
        self.program_counter.position
    }
}
