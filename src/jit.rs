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

use std::array;
use std::collections::HashMap;
use std::io;
use std::io::Read;
use std::io::Write;
use std::rc::Rc;

use rand::seq::SliceRandom;
use rand::SeedableRng;

use crate::scan_next;
use crate::Direction;
use crate::Error;
use crate::Executer;
use crate::FastHasher;
use crate::Grid;
use crate::Int;
use crate::Interpreter;
use crate::Position;
use crate::GRID_HEIGHT;
use crate::GRID_WIDTH;
use crate::PC;

const BASIC_BLOCK_SIZE_LIMIT: usize = 2048;

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

#[derive(Clone, Debug)]
struct BasicBlock {
    entry_point: PC,
    bytecode: Vec<Operation>,
    cf_decision: ControlFlowDecision,
}

impl std::hash::Hash for BasicBlock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.entry_point.hash(state);
    }
}

impl BasicBlock {
    fn new(entry_point: PC) -> Self {
        Self {
            entry_point,
            bytecode: Vec::new(),
            cf_decision: ControlFlowDecision::EndProgram,
        }
    }

    fn execute(&self, jit: &mut JustInTimeCompiler<'_>) -> Result<ControlFlowDecision, Error> {
        for op in &self.bytecode {
            match op {
                Operation::PushConstant(number) => jit.stack.push(*number),
                Operation::Duplicate => {
                    let top = jit.stack.pop().unwrap_or_default();
                    jit.stack.push(top);
                    jit.stack.push(top);
                }
                Operation::Swap => {
                    let first = jit.stack.pop().unwrap_or_default();
                    let second = jit.stack.pop().unwrap_or_default();
                    jit.stack.push(first);
                    jit.stack.push(second);
                }
                Operation::Drop => {
                    jit.stack.pop();
                }
                Operation::Binary(binary) => {
                    let b = jit.stack.pop().unwrap_or_default();
                    let a = jit.stack.pop().unwrap_or_default();
                    jit.stack.push(binary.call(a, b));
                }
                Operation::Negate => {
                    let top = jit.stack.pop().unwrap_or_default();
                    jit.stack.push(if top == 0 { 1 } else { 0 });
                }
                Operation::Input(IOMode::Ascii) => {
                    // To my knowledge, the EOF behavior of Befunge-93 input is documented nowhere.
                    // jsFunge (and probably all others) will retrieve -1 on EOF, and not a null character.
                    // Conveniently, 0xff is not a valid byte for UTF-8 coding, so we can use it here.
                    let mut ascii = 0xff;
                    jit.input
                        .read_exact(std::slice::from_mut(&mut ascii))
                        .map_or_else(
                            |e| {
                                if e.kind() == io::ErrorKind::UnexpectedEof {
                                    Ok(())
                                } else {
                                    Err(e)
                                }
                            },
                            |_| Ok(()),
                        )?;
                    jit.stack
                        .push(if ascii != 0xff { ascii.into() } else { -1 });
                }
                Operation::Input(IOMode::Decimal) => {
                    let number = scan_next(&mut jit.input)?;
                    jit.stack.push(number);
                }
                Operation::Output(IOMode::Ascii) => {
                    let top = jit.stack.pop().unwrap_or_default();
                    let ascii =
                        char::try_from(u32::try_from(top).map_err(|_| Error::NonAscii(top))?)
                            .map_err(|_| Error::NonAscii(top))?;
                    if !ascii.is_ascii() {
                        return Err(Error::NonAscii(ascii as Int));
                    } else {
                        jit.output.write_all(&[ascii as u8])?;
                    }
                }
                Operation::Output(IOMode::Decimal) => {
                    let top = jit.stack.pop().unwrap_or_default();
                    write!(jit.output, "{} ", top)?;
                }
                Operation::GetValue => {
                    let y = jit.stack.pop().unwrap_or_default();
                    let x = jit.stack.pop().unwrap_or_default();
                    jit.stack.push(
                        if !(0..GRID_WIDTH as Int).contains(&x)
                            || !(0..GRID_HEIGHT as Int).contains(&y)
                        {
                            0
                        } else {
                            // make sure to retain signedness, even though ASCII is not really signed
                            jit.program_grid[y as usize][x as usize] as i8 as Int
                        },
                    );
                }
                Operation::SetValue { pc_after } => {
                    let y = jit.stack.pop().unwrap_or_default();
                    let x = jit.stack.pop().unwrap_or_default();
                    let position = Position::new(x as _, y as _);
                    let value = jit.stack.pop().unwrap_or_default();
                    if (0..GRID_WIDTH as Int).contains(&x) && (0..GRID_HEIGHT as Int).contains(&y) {
                        jit.program_grid[y as usize][x as usize] = value as u8;
                    }
                    let invalidated_entries = jit.invalidate_bytecode(position);
                    // If we were invalidated, exit bytecode immediately and continue with the next PC.
                    if invalidated_entries.contains(&self.entry_point.position) {
                        return Ok(ControlFlowDecision::Jump(*pc_after));
                    }
                }
            }
        }
        Ok(self.cf_decision)
    }
}

/// A map from grid cells to all entry points of basic blocks that pass through the cell.
type GridBlockMap = [[Vec<PC>; GRID_WIDTH]; GRID_HEIGHT];

pub struct JustInTimeCompiler<'rw> {
    basic_blocks: HashMap<PC, Rc<BasicBlock>, FastHasher>,
    program_grid: Grid,
    grid_block_map: GridBlockMap,
    stack: Vec<Int>,
    program_counter: PC,
    // I/O
    input: Box<dyn Read + 'rw>,
    output: Box<dyn Write + 'rw>,
    rng: rand::rngs::SmallRng,
    // Statistics
    basic_block_compiles: usize,
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
            basic_blocks: Default::default(),
            stack: Vec::new(),
            program_grid: parsed_grid,
            grid_block_map: array::from_fn(|_| array::from_fn(|_| Vec::new())),
            program_counter: PC::default(),
            input,
            output,
            rng: rand::rngs::SmallRng::seed_from_u64(start.to_bits()),
            basic_block_compiles: 0,
        })
    }

    fn compile_basic_block_from(
        start_pc: PC,
        grid: &Grid,
    ) -> Result<(BasicBlock, Vec<Position>), Error> {
        let mut basic_block = BasicBlock::new(start_pc);

        let mut current_pc = start_pc;
        let mut string_mode = false;
        let mut pcs = Vec::new();
        loop {
            let current_command =
                grid[current_pc.position.y as usize][current_pc.position.x as usize];
            pcs.push(current_pc.position);
            if string_mode {
                if current_command == b'"' {
                    string_mode = false;
                } else {
                    basic_block
                        .bytecode
                        .push(Operation::PushConstant(current_command as _));
                }
                current_pc.step();
                current_pc.constrain();
            } else {
                // Automatically stop a basic block after reaching a certain number of basic blocks.
                // This is a crude infinite loop detection that performs better than checking all previously visited PCs.
                // If the loop contains no actual commands (except unconditional PC redirects), we still get stuck, but since the program doesn't do anything in that case, the behavior is correct.
                if basic_block.bytecode.len() > BASIC_BLOCK_SIZE_LIMIT {
                    basic_block.cf_decision = ControlFlowDecision::Jump(current_pc);
                    break;
                }
                // FIXME: Check if we reached any start PCs of any other basic block.
                match current_command {
                    // PC redirection
                    b'>' => {
                        current_pc.direction = Direction::Right;
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'<' => {
                        current_pc.direction = Direction::Left;
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'^' => {
                        current_pc.direction = Direction::Up;
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'v' => {
                        current_pc.direction = Direction::Down;
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'?' => {
                        let decision = ControlFlowDecision::Random {
                            choices: [
                                Direction::Up,
                                Direction::Down,
                                Direction::Left,
                                Direction::Right,
                            ]
                            .map(|direction| PC {
                                position: current_pc.position + direction,
                                direction,
                            }),
                        };
                        basic_block.cf_decision = decision;
                        break;
                    }
                    b'#' => {
                        current_pc.step();
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b' ' => {
                        current_pc.step();
                        current_pc.constrain();
                    }
                    // Literals
                    b'"' => {
                        string_mode = true;
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'0'..=b'9' => {
                        let number = current_command - b'0';
                        basic_block
                            .bytecode
                            .push(Operation::PushConstant(number as _));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    // Stack ops
                    b':' => {
                        basic_block.bytecode.push(Operation::Duplicate);
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'\\' => {
                        basic_block.bytecode.push(Operation::Swap);
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'$' => {
                        basic_block.bytecode.push(Operation::Drop);
                        current_pc.step();
                        current_pc.constrain();
                    }
                    // Math ops
                    b'+' => {
                        basic_block
                            .bytecode
                            .push(Operation::Binary(BinaryOperation::Add));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'-' => {
                        basic_block
                            .bytecode
                            .push(Operation::Binary(BinaryOperation::Subtract));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'*' => {
                        basic_block
                            .bytecode
                            .push(Operation::Binary(BinaryOperation::Multiply));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'/' => {
                        basic_block
                            .bytecode
                            .push(Operation::Binary(BinaryOperation::Divide));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'%' => {
                        basic_block
                            .bytecode
                            .push(Operation::Binary(BinaryOperation::Remainder));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'!' => {
                        basic_block.bytecode.push(Operation::Negate);
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'`' => {
                        basic_block
                            .bytecode
                            .push(Operation::Binary(BinaryOperation::Greater));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    // I/O
                    b',' => {
                        basic_block.bytecode.push(Operation::Output(IOMode::Ascii));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'.' => {
                        basic_block
                            .bytecode
                            .push(Operation::Output(IOMode::Decimal));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'~' => {
                        basic_block.bytecode.push(Operation::Input(IOMode::Ascii));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'&' => {
                        basic_block.bytecode.push(Operation::Input(IOMode::Decimal));
                        current_pc.step();
                        current_pc.constrain();
                    }
                    // Conditionals
                    b'_' => {
                        let decision = ControlFlowDecision::Branch {
                            true_target: PC {
                                position: current_pc.position + Direction::Left,
                                direction: Direction::Left,
                            },
                            false_target: PC {
                                position: current_pc.position + Direction::Right,
                                direction: Direction::Right,
                            },
                        };
                        basic_block.cf_decision = decision;
                        break;
                    }
                    b'|' => {
                        let decision = ControlFlowDecision::Branch {
                            true_target: PC {
                                position: current_pc.position + Direction::Up,
                                direction: Direction::Up,
                            },
                            false_target: PC {
                                position: current_pc.position + Direction::Down,
                                direction: Direction::Down,
                            },
                        };
                        basic_block.cf_decision = decision;
                        break;
                    }
                    b'@' => {
                        basic_block.cf_decision = ControlFlowDecision::EndProgram;
                        break;
                    }
                    // Self-modification
                    b'g' => {
                        basic_block.bytecode.push(Operation::GetValue);
                        current_pc.step();
                        current_pc.constrain();
                    }
                    b'p' => {
                        current_pc.step();
                        current_pc.constrain();
                        basic_block.bytecode.push(Operation::SetValue {
                            pc_after: current_pc,
                        });
                    }
                    _ => return Err(Error::IllegalCommand(current_command)),
                }
            }
        }
        // TODO: Output compiled basic block if CLI flag is on

        Ok((basic_block, pcs))
    }

    fn ensure_basic_block(&mut self) -> Result<Rc<BasicBlock>, Error> {
        let basic_block_entry = self.basic_blocks.entry(self.program_counter);
        match basic_block_entry {
            std::collections::hash_map::Entry::Occupied(basic_block) => {
                Ok(basic_block.get().clone())
            }
            std::collections::hash_map::Entry::Vacant(_) => {
                let (basic_block, elements) =
                    Self::compile_basic_block_from(self.program_counter, &self.program_grid)?;
                let basic_block = Rc::new(basic_block);
                self.basic_block_compiles += 1;
                basic_block_entry.or_insert(basic_block.clone());

                for cell in elements {
                    self.grid_block_map[cell.y as usize][cell.x as usize]
                        .push(self.program_counter);
                }

                Ok(basic_block)
            }
        }
    }

    /// Returns the entry point positions of invalidated basic blocks.
    fn invalidate_bytecode(&mut self, cell: Position) -> Vec<Position> {
        let invalid_entry_points = &mut self.grid_block_map[cell.y as usize][cell.x as usize];
        for invalid_entry_point in invalid_entry_points.iter() {
            self.basic_blocks.remove(invalid_entry_point);
        }

        invalid_entry_points
            .drain(..)
            .map(|pc| pc.position)
            .collect()
    }
}

impl<'rw> Executer for JustInTimeCompiler<'rw> {
    fn run_forever(&mut self) -> Result<(), Error> {
        loop {
            let basic_block = self.ensure_basic_block()?;
            let result = basic_block.execute(self);
            match result {
                Ok(cf_decision) => match cf_decision {
                    ControlFlowDecision::Jump(target) => {
                        self.program_counter = target;
                    }
                    ControlFlowDecision::Branch {
                        true_target,
                        false_target,
                    } => {
                        let result = self.stack.pop().unwrap_or_default();
                        self.program_counter = if result == 0 {
                            false_target
                        } else {
                            true_target
                        };
                    }
                    ControlFlowDecision::EndProgram => return Ok(()),
                    ControlFlowDecision::Random { choices } => {
                        let choice = *choices.choose(&mut self.rng).unwrap();
                        self.program_counter = choice;
                    }
                },
                Err(Error::ProgramEnd) => return Ok(()),
                Err(why) => return Err(why),
            }
        }
    }

    fn steps(&self) -> usize {
        self.basic_block_compiles
    }

    fn position(&self) -> Position {
        self.program_counter.position
    }
}
