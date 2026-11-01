# Examples and starters

What the simulation opens with, generated from the program itself. Each is read-only; Save copies one into a project of your own.

## Examples

Short programs that teach one thing each.

- **Blink (TON)**
- **Start/stop latch**
- **Counter**
- **Traffic light**
- **Line of stations (nested blocks)**

## Machines

Worked programs, each with a plant model so it runs with nothing wired; press `start_pb` to start one.

| project | | what it shows |
|---|---|---|
| `conveyor-sorter` | **Conveyor sorter** | Measures parts at a photo eye and diverts the long ones, with belt tracking and a jam alarm |
| `pump-station` | **Pump station** | Two pumps on a wet well, duty and standby, run from a level transmitter |
| `batch-mixer` | **Batch mixing tank** | Fill, heat while agitating, hold and drain, with interlocks on the heater |
| `pick-and-place` | **Pick and place** | A three-cylinder pneumatic arm with end switches and travel timeouts |
| `bottle-filler` | **Bottle filler and capper** | An indexing conveyor with a timed fill head and a cap press |
| `air-compressors` | **Air compressor staging** | Three compressors staged on header pressure, with a rotating lead |
| `air-handler` | **Air handling unit** | Damper, fan with airflow proof, PID on the heating coil, freeze stat |

## Templates

A skeleton to begin a program of that shape from.

| project | | what it shows |
|---|---|---|
| `discrete-template` | **Discrete machine** | A motor, a cylinder and a step sequence, ready to rename and extend |
| `process-template` | **Process loop** | A scaled measurement, a PID loop and two pumps sharing the duty |
