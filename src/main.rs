#![doc = include_str!("../README.md")]

use argh::FromArgValue;
use argh::FromArgs;
use rand::distributions::Distribution;
use rand::distributions::Standard;
use rand::Rng;
use rand::SeedableRng;
use std::fs::File;
use std::hint::unreachable_unchecked;
use std::io;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::slice;
use std::str::FromStr;

#[cfg(test)]
mod test;

/// "each cell of the stack can hold as much as a C language signed long int on the same platform."
type Int = std::ffi::c_long;

const GRID_HEIGHT: usize = 25;
const GRID_WIDTH: usize = 80;
type Line = [u8; GRID_WIDTH];
type Grid = [Line; GRID_HEIGHT];
type Stack = Vec<Int>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LanguageStandard {
    Befunge93,
    #[default]
    Befunge98,
}

impl FromArgValue for LanguageStandard {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        Ok(match value {
            "93" => Self::Befunge93,
            "98" => Self::Befunge98,
            _ => {
                return Err(
                    "unknown Befunge language standard, possible values are [98, 93]".to_string(),
                )
            }
        })
    }
}

#[derive(FromArgs)]
/// Befunge-93 interpreter.
struct Arguments {
    /// input file to read
    #[argh(positional)]
    input: PathBuf,
    /// collect and show performance metrics
    #[argh(switch, short = 'p')]
    show_performance: bool,
    /// language standard to use, for future compatibility. default: 98
    #[argh(option, short = 's', default = "LanguageStandard::default()")]
    language_standard: LanguageStandard,
    /// file to use as stdin for the program; particularly useful with self-interpreters
    #[argh(option, short = 'i')]
    stdin: Option<PathBuf>,
}

type Position = glam::I64Vec2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Direction {
    Up,
    Down,
    Left,
    #[default]
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct PC {
    position: Position,
    direction: Direction,
}

impl PC {
    pub fn step(&mut self) {
        self.position += self.direction;
    }

    pub fn constrain(&mut self) {
        if !(0..GRID_WIDTH).contains(&(self.position.x as usize)) {
            self.position.x = (self.position.x + GRID_WIDTH as i64) % GRID_WIDTH as i64;
        }
        if !(0..GRID_HEIGHT).contains(&(self.position.y as usize)) {
            self.position.y = (self.position.y + GRID_HEIGHT as i64) % GRID_HEIGHT as i64;
        }
    }
}

impl Distribution<Direction> for Standard {
    fn sample<R: rand::prelude::Rng + ?Sized>(&self, rng: &mut R) -> Direction {
        match rng.gen_range(0..4) {
            0 => Direction::Up,
            1 => Direction::Down,
            2 => Direction::Left,
            3 => Direction::Right,
            _ => unsafe { unreachable_unchecked() },
        }
    }
}

impl std::ops::Add<Direction> for Position {
    type Output = Self;

    fn add(self, rhs: Direction) -> Self::Output {
        self + Position::from(match rhs {
            Direction::Up => (0, -1),
            Direction::Down => (0, 1),
            Direction::Left => (-1, 0),
            Direction::Right => (1, 0),
        })
    }
}

impl std::ops::AddAssign<Direction> for Position {
    fn add_assign(&mut self, rhs: Direction) {
        *self = *self + rhs;
    }
}

/// Anything executing a Befunge program.
trait Executer {
    /// Run the executer's main loop.
    fn run_forever(&mut self) -> Result<(), Error>;
    fn steps(&self) -> usize;
    fn position(&self) -> Position;
}

/// The Befunge interpreter.
/// Lifetime parameter is for the I/O structures, which must outlive the interpreter.
struct Interpreter<'rw> {
    // Data and program
    program_grid: Grid,
    // Core state
    stack: Stack,
    string_mode: bool,
    program_counter: PC,
    // I/O
    input: Box<dyn Read + 'rw>,
    output: Box<dyn Write + 'rw>,
    rng: rand::rngs::SmallRng,
    // Debugging
    steps: usize,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Input/Output error")]
    Io(#[from] io::Error),
    #[error("Grid size {0} x {1} invalid")]
    InvalidGridSize(usize, usize),
    #[error("Non-ASCII character \"{0:x}\" in input")]
    NonAscii(Int),
    #[error("Illegal command '{}' ({command:x})", *.command as char)]
    IllegalCommand { command: u8 },
    #[error("Program terminated normally")]
    ProgramEnd,
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(_), Self::Io(_)) => false,
            (Self::InvalidGridSize(l0, l1), Self::InvalidGridSize(r0, r1)) => l0 == r0 && l1 == r1,
            (Self::NonAscii(l0), Self::NonAscii(r0)) => l0 == r0,
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}

/// Modified from text_io's implementation to accept Read instead of iterators as an input.
pub fn scan_next<T>(input: &mut impl Read) -> Result<T, io::Error>
where
    T: FromStr,
    <T as FromStr>::Err: std::error::Error + Send + Sync + 'static,
{
    let mut buffer = b' ';
    while (buffer as char).is_whitespace() || buffer == 0 {
        let result = input.read_exact(slice::from_mut(&mut buffer));
        match result {
            Ok(_) => {}
            Err(why) if why.kind() == ErrorKind::UnexpectedEof => buffer = 0,
            Err(why) => return Err(why),
        }
    }

    let mut raw = Vec::new();
    while !(buffer as char).is_whitespace() && buffer != 0 {
        raw.push(buffer);
        let result = input.read_exact(slice::from_mut(&mut buffer));
        match result {
            Ok(_) => {}
            Err(why) if why.kind() == ErrorKind::UnexpectedEof => buffer = 0,
            Err(why) => return Err(why),
        }
    }

    match String::from_utf8(raw) {
        Ok(s) => {
            FromStr::from_str(&s).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
        Err(_) => Err(io::Error::from(io::ErrorKind::InvalidData)),
    }
}

impl<'rw> Interpreter<'rw> {
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
        let parsed_grid = Self::parse_grid(grid)?;
        Ok(Self {
            stack: Stack::new(),
            program_grid: parsed_grid,
            string_mode: false,
            program_counter: PC::default(),
            input,
            output,
            rng: rand::rngs::SmallRng::seed_from_u64(start.to_bits()),
            steps: 0,
        })
    }

    pub fn parse_grid(grid: &str) -> Result<Grid, Error> {
        let lines = grid
            .lines()
            .map(|line| {
                line.chars()
                    .map(|x| {
                        if x.is_ascii() {
                            Ok(x as u8)
                        } else {
                            Err(Error::NonAscii(x as Int))
                        }
                    })
                    .collect::<Result<Vec<_>, Error>>()
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let height = lines.len();
        let width = lines.first().map_or(0, |v| v.len());
        if height > GRID_HEIGHT || width > GRID_WIDTH {
            return Err(Error::InvalidGridSize(width, height));
        }
        Ok(Grid::try_from({
            let mut grid = lines
                .into_iter()
                .map(|mut line| {
                    line.resize(GRID_WIDTH, b' ');
                    Line::try_from(line).unwrap()
                })
                .collect::<Vec<_>>();
            grid.resize(GRID_HEIGHT, [b' '; GRID_WIDTH]);
            grid
        })
        .unwrap())
    }

    pub fn run_step(&mut self) -> Result<(), Error> {
        macro_rules! move_pc {
            () => {
                self.program_counter.step();
                self.program_counter.constrain();
            };
        }
        self.steps += 1;

        let current_char = self.program_grid[self.program_counter.position.y as usize]
            [self.program_counter.position.x as usize];
        if self.string_mode {
            if current_char == b'"' {
                self.string_mode = false;
            } else {
                self.stack.push(current_char as Int);
            }
            move_pc!();
            Ok(())
        } else {
            match current_char {
                // PC redirection
                b'>' => {
                    self.program_counter.direction = Direction::Right;
                    move_pc!();
                    Ok(())
                }
                b'<' => {
                    self.program_counter.direction = Direction::Left;
                    move_pc!();
                    Ok(())
                }
                b'^' => {
                    self.program_counter.direction = Direction::Up;
                    move_pc!();
                    Ok(())
                }
                b'v' => {
                    self.program_counter.direction = Direction::Down;
                    move_pc!();
                    Ok(())
                }
                b'?' => {
                    self.program_counter.direction = self.rng.gen();
                    move_pc!();
                    Ok(())
                }
                b'#' => {
                    move_pc!();
                    move_pc!();
                    Ok(())
                }
                b' ' => {
                    move_pc!();
                    Ok(())
                }
                // Literals
                b'"' => {
                    self.string_mode = true;
                    move_pc!();
                    Ok(())
                }
                b'0'..=b'9' => {
                    let number = current_char - b'0';
                    self.stack.push(number as Int);
                    move_pc!();
                    Ok(())
                }
                // Stack ops
                b':' => {
                    let top = self.stack.pop().unwrap_or_default();
                    self.stack.push(top);
                    self.stack.push(top);
                    move_pc!();
                    Ok(())
                }
                b'\\' => {
                    let top = self.stack.pop().unwrap_or_default();
                    let second = self.stack.pop().unwrap_or_default();
                    self.stack.push(top);
                    self.stack.push(second);
                    move_pc!();
                    Ok(())
                }
                b'$' => {
                    let _ = self.stack.pop();
                    move_pc!();
                    Ok(())
                }
                // Math ops
                b'+' => {
                    let b = self.stack.pop().unwrap_or_default();
                    let a = self.stack.pop().unwrap_or_default();
                    self.stack.push(a.wrapping_add(b));
                    move_pc!();
                    Ok(())
                }
                b'-' => {
                    let b = self.stack.pop().unwrap_or_default();
                    let a = self.stack.pop().unwrap_or_default();
                    self.stack.push(a.wrapping_sub(b));
                    move_pc!();
                    Ok(())
                }
                b'*' => {
                    let b = self.stack.pop().unwrap_or_default();
                    let a = self.stack.pop().unwrap_or_default();
                    self.stack.push(a.wrapping_mul(b));
                    move_pc!();
                    Ok(())
                }
                b'/' => {
                    let b = self.stack.pop().unwrap_or_default();
                    let a = self.stack.pop().unwrap_or_default();
                    self.stack.push(a.wrapping_div(b));
                    move_pc!();
                    Ok(())
                }
                b'%' => {
                    let b = self.stack.pop().unwrap_or_default();
                    let a = self.stack.pop().unwrap_or_default();
                    self.stack.push(a.wrapping_rem(b));
                    move_pc!();
                    Ok(())
                }
                b'!' => {
                    let b = self.stack.pop().unwrap_or_default();
                    self.stack.push(if b == 0 { 1 } else { 0 });
                    move_pc!();
                    Ok(())
                }
                b'`' => {
                    let b = self.stack.pop().unwrap_or_default();
                    let a = self.stack.pop().unwrap_or_default();
                    self.stack.push(if a > b { 1 } else { 0 });
                    move_pc!();
                    Ok(())
                }
                // I/O
                b',' => {
                    let top = self.stack.pop().unwrap_or_default();
                    let ascii =
                        char::try_from(u32::try_from(top).map_err(|_| Error::NonAscii(top))?)
                            .map_err(|_| Error::NonAscii(top))?;
                    if !ascii.is_ascii() {
                        Err(Error::NonAscii(ascii as Int))
                    } else {
                        self.output.write_all(&[ascii as u8])?;
                        move_pc!();
                        Ok(())
                    }
                }
                b'.' => {
                    let top = self.stack.pop().unwrap_or_default();
                    write!(self.output, "{} ", top)?;
                    move_pc!();
                    Ok(())
                }
                b'~' => {
                    // To my knowledge, the EOF behavior of Befunge-93 input is documented nowhere.
                    // jsFunge (and probably all others) will retrieve -1 on EOF, and not a null character.
                    // Conveniently, 0xff is not a valid byte for UTF-8 coding, so we can use it here.
                    let mut ascii = 0xff;
                    self.input
                        .read_exact(slice::from_mut(&mut ascii))
                        .map_or_else(
                            |e| {
                                if e.kind() == ErrorKind::UnexpectedEof {
                                    Ok(())
                                } else {
                                    Err(e)
                                }
                            },
                            |_| Ok(()),
                        )?;
                    self.stack
                        .push(if ascii != 0xff { ascii.into() } else { -1 });
                    move_pc!();
                    Ok(())
                }
                b'&' => {
                    let number = scan_next(&mut self.input)?;
                    self.stack.push(number);
                    move_pc!();
                    Ok(())
                }
                // Conditionals
                b'_' => {
                    let top = self.stack.pop().unwrap_or_default();
                    self.program_counter.direction = if top == 0 {
                        Direction::Right
                    } else {
                        Direction::Left
                    };
                    move_pc!();
                    Ok(())
                }
                b'|' => {
                    let top = self.stack.pop().unwrap_or_default();
                    self.program_counter.direction = if top == 0 {
                        Direction::Down
                    } else {
                        Direction::Up
                    };
                    move_pc!();
                    Ok(())
                }
                // Self-modification
                b'g' => {
                    let y = self.stack.pop().unwrap_or_default();
                    let x = self.stack.pop().unwrap_or_default();
                    self.stack.push(
                        if !(0..GRID_WIDTH as Int).contains(&x)
                            || !(0..GRID_HEIGHT as Int).contains(&y)
                        {
                            0
                        } else {
                            // make sure to retain signedness, even though ASCII is not really signed
                            self.program_grid[y as usize][x as usize] as i8 as Int
                        },
                    );
                    move_pc!();
                    Ok(())
                }
                b'p' => {
                    let y = self.stack.pop().unwrap_or_default();
                    let x = self.stack.pop().unwrap_or_default();
                    let value = self.stack.pop().unwrap_or_default();
                    if (0..GRID_WIDTH as Int).contains(&x) && (0..GRID_HEIGHT as Int).contains(&y) {
                        self.program_grid[y as usize][x as usize] = value as u8;
                    }
                    move_pc!();
                    Ok(())
                }
                // Misc
                b'@' => Err(Error::ProgramEnd),
                _ => Err(Error::IllegalCommand {
                    command: current_char,
                }),
            }
        }
    }
}

impl<'rw> Executer for Interpreter<'rw> {
    fn run_forever(&mut self) -> Result<(), Error> {
        loop {
            let result = self.run_step();
            if result.as_ref().is_err_and(|e| e == &Error::ProgramEnd) {
                return Ok(());
            }
            result?;
        }
    }

    fn steps(&self) -> usize {
        self.steps
    }

    fn position(&self) -> Position {
        self.program_counter.position
    }
}

fn run_interpreter(args: Arguments) -> Result<(), Error> {
    let mut grid: String = String::new();
    if args.input == Path::new("-") {
        io::stdin().read_to_string(&mut grid)?;
    } else {
        File::open(args.input)?.read_to_string(&mut grid)?;
    }
    let mut interpreter = Box::new(args.stdin.map_or_else(
        || Interpreter::new(&grid),
        |stdin| {
            Interpreter::new_with_io(&grid, Box::new(File::open(stdin)?), Box::new(io::stdout()))
        },
    )?);

    let start = std::time::Instant::now();
    let result = interpreter.run_forever();
    let end = std::time::Instant::now();

    match result {
        Ok(_) => {}
        Err(ref why) => eprintln!("error at {}: {}", interpreter.position(), why),
    }

    if args.show_performance {
        let time = end - start;
        let time_per_step = time / interpreter.steps() as u32;
        println!();
        println!(
            "execution took {:?}, {} steps, {:?} / step, {:.3} Msteps/s",
            time,
            interpreter.steps(),
            time_per_step,
            1_000.0 / time_per_step.as_nanos() as f64
        );
    }

    if result.is_err() {
        std::process::exit(1);
    }

    Ok(())
}

fn main() {
    let args: Arguments = argh::from_env();

    if args.language_standard != LanguageStandard::Befunge93 {
        eprintln!("only Befunge-93 is currently supported");
        std::process::exit(1);
    }

    run_interpreter(args).unwrap();
}
