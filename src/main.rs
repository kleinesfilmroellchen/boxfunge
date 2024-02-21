#![doc = include_str!("../README.md")]

use argh::FromArgValue;
use argh::FromArgs;
use random::Source;
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
use std::time::SystemTime;

#[cfg(test)]
mod test;

/// "each cell of the stack can hold as much as a C language signed long int on the same platform."
type Int = std::ffi::c_long;

const GRID_HEIGHT: usize = 25;
const GRID_WIDTH: usize = 80;
const GRID_SIZE: Position = Position::new(GRID_WIDTH as i64, GRID_HEIGHT as i64);
type Line = [u8; GRID_WIDTH];
type Grid = [Line; GRID_HEIGHT];

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
}

type Position = glam::I64Vec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl random::Value for Direction {
    fn read<S>(source: &mut S) -> Self
    where
        S: Source,
    {
        let random = source.read_u64();
        match random % 4 {
            0 => Self::Up,
            1 => Self::Down,
            2 => Self::Left,
            3 => Self::Right,
            // SAFETY: i hope math isn't broken
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

/// The Befunge interpreter.
/// Lifetime parameter is for the I/O structures, which must outlive the interpreter.
struct Interpreter<'rw> {
    // Data and program
    program_grid: Grid,
    // Core state
    stack: Vec<Int>,
    string_mode: bool,
    program_counter: Position,
    direction: Direction,
    // I/O
    input: Box<dyn Read + 'rw>,
    output: Box<dyn Write + 'rw>,
    rng: random::Default,
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
    while (buffer as char).is_whitespace() {
        let result = input.read_exact(slice::from_mut(&mut buffer));
        match result {
            Ok(_) => {}
            Err(why) if why.kind() == ErrorKind::UnexpectedEof => break,
            Err(why) => return Err(why),
        }
    }

    let mut raw = Vec::new();
    while !(buffer as char).is_whitespace() {
        raw.push(buffer);
        let result = input.read_exact(slice::from_mut(&mut buffer));
        match result {
            Ok(_) => {}
            Err(why) if why.kind() == ErrorKind::UnexpectedEof => break,
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
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let parsed_grid = Self::parse_grid(grid)?;
        Ok(Self {
            stack: Vec::new(),
            program_grid: parsed_grid,
            string_mode: false,
            program_counter: Position::ZERO,
            input,
            output,
            direction: Direction::Right,
            rng: random::Default::new([start.to_bits(), start.to_bits()]),
            steps: 0,
        })
    }

    fn parse_grid(grid: &str) -> Result<Grid, Error> {
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
        let width = lines.get(0).map_or(0, |v| v.len());
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

    pub fn run_forever(&mut self) -> Result<(), Error> {
        loop {
            let result = self.run_step();
            if result.as_ref().is_err_and(|e| e == &Error::ProgramEnd) {
                return Ok(());
            }
            result?;
        }
    }

    pub fn run_step(&mut self) -> Result<(), Error> {
        macro_rules! move_pc {
            () => {
                self.program_counter += self.direction;
                // FIXME: Hack to work around -1 % 80 != 79
                self.program_counter += GRID_SIZE;
                self.program_counter %= GRID_SIZE;
            };
        }
        self.steps += 1;

        let current_char =
            self.program_grid[self.program_counter.y as usize][self.program_counter.x as usize];
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
                    self.direction = Direction::Right;
                    move_pc!();
                    Ok(())
                }
                b'<' => {
                    self.direction = Direction::Left;
                    move_pc!();
                    Ok(())
                }
                b'^' => {
                    self.direction = Direction::Up;
                    move_pc!();
                    Ok(())
                }
                b'v' => {
                    self.direction = Direction::Down;
                    move_pc!();
                    Ok(())
                }
                b'?' => {
                    self.direction = self.rng.read();
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
                    self.stack
                        .push(self.stack.last().cloned().unwrap_or_default());
                    move_pc!();
                    Ok(())
                }
                b'\\' => {
                    // TODO:
                    // let last_chunk = self.stack.last_chunk_mut::<2>();
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
                    let text = top.to_string();
                    self.output.write(text.as_bytes())?;
                    move_pc!();
                    Ok(())
                }
                b'~' => {
                    let mut ascii = Vec::from([0]);
                    self.input.read_exact(&mut ascii)?;
                    self.stack.push(ascii[0].into());
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
                    self.direction = if top == 0 {
                        Direction::Right
                    } else {
                        Direction::Left
                    };
                    move_pc!();
                    Ok(())
                }
                b'|' => {
                    let top = self.stack.pop().unwrap_or_default();
                    self.direction = if top == 0 {
                        Direction::Down
                    } else {
                        Direction::Up
                    };
                    move_pc!();
                    Ok(())
                }
                // Misc
                b'@' => Err(Error::ProgramEnd),
                _ => todo!(
                    "unimplemented command '{}' ({:02x})",
                    current_char as char,
                    current_char
                ),
            }
        }
    }
}

fn main() -> Result<(), Error> {
    let args: Arguments = argh::from_env();

    if args.language_standard != LanguageStandard::Befunge93 {
        eprintln!("only Befunge-93 is currently supported");
        std::process::exit(1);
    }

    let mut grid: String = String::new();
    if args.input == Path::new("-") {
        io::stdin().read_to_string(&mut grid)?;
    } else {
        File::open(args.input)?.read_to_string(&mut grid)?;
    }
    let mut interpreter = Box::new(Interpreter::new(&grid)?);

    let start = std::time::Instant::now();
    interpreter.run_forever()?;
    let end = std::time::Instant::now();

    if args.show_performance {
        let time = end - start;
        let time_per_step = time / interpreter.steps as u32;
        println!();
        println!(
            "execution took {:?}, {} steps, {:?} / step",
            time, interpreter.steps, time_per_step
        );
    }

    Ok(())
}
