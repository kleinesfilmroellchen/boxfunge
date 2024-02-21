#![doc = include_str!("../README.md")]

use std::io;
use std::io::Read;
use std::io::Write;

/// "each cell of the stack can hold as much as a C language signed long int on the same platform."
type Int = std::ffi::c_long;

const GridHeight: usize = 25;
const GridWidth: usize = 80;
type Line = [u8; GridWidth];
type Grid = [Line; GridHeight];

struct Interpreter {
    stack: Vec<Int>,
    program_grid: Grid,
    string_mode: bool,
    program_counter: (usize, usize),
    input: Box<dyn Read>,
    output: Box<dyn Write>,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Input/Output error")]
    Io(#[from] io::Error),
    #[error("Grid size {0} x {1} invalid")]
    InvalidGridSize(usize, usize),
    #[error("Non-ASCII character \"{0}\" in input")]
    NonAscii(char),
}

impl Interpreter {
    pub fn new(grid: &str) -> Result<Self, Error> {
        let parsed_grid = Self::parse_grid(grid)?;
        todo!()
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
                            Err(Error::NonAscii(x))
                        }
                    })
                    .collect::<Result<Vec<_>, Error>>()
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let height = lines.len();
        let width = lines.get(0).map_or(0, |v| v.len());
        if height > GridHeight || width > GridWidth {
            return Err(Error::InvalidGridSize(width, height));
        }
        Ok(Grid::try_from(lines.into_iter().map(|line| Line::try_from(line).unwrap()).collect::<Vec<_>>()).unwrap())
    }
}

fn main() {}
