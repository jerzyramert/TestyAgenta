Comprehensive Technical Specification: Yacht Polar Formats and Routing Pre-processing
This specification defines the rigorous data formats, structural schemas, unit conventions, and mathematical fallback rules required to implement STEP-05 routing engines without ambiguity.

1. Unit Conventions and Dimensionality
To eliminate ambiguity in tabular data:
True Wind Speed (TWS): Knots (kt), floating-point or integer values.
True Wind Angle (TWA): Degrees ($^\circ$), absolute value from 0 (head-to-wind) to 180 (dead downwind). Standard polars assume symmetry between port and starboard unless explicitly indexed with a sign (e.g., negative for port, positive for starboard).
Boat Speed (BS): Knots (kt), floating-point values representing steady-state velocity made through the water (VMW).

2. Standardized .pol Format Rules and Parsing Specification
The .pol format (Expedition/qtVlm style) is parsed according to the following strict grammar:
Comment Lines: Any line starting with //, #, or whitespace followed by these symbols must be ignored. Empty lines are skipped.
Header Identification: The first non-comment line containing numerical values following a text token (or directly on a dedicated line) defines the TWS axis vector.
Example: TWS 6 8 10 12 14 16 20 24 -> TWS Axis: [6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 20.0, 24.0]
Row Data Structure: Subsequent rows begin with the TWA value (first column), followed by |TWS| floating-point values representing Boat Speed (BS) in knots.
Missing Values / Extrapolation Limits: Cells containing invalid tokens (like 99.9, -1, or NaN) represent unreachable states or maneuvers restricted by sail configurations.

3. Normative ORC JSON Schema
When integrating official ORC (Offshore Racing Congress) VPP outputs, systems must support the following JSON structure encapsulating multiple sail configurations (sails indices) and wind matrices.
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "ORCYachtPolar",
  "type": "object",
  "properties": {
    "metadata": {
      "type": "object",
      "properties": {
        "boatName": { "type": "string" },
        "source": { "type": "string" },
        "unitTWS": { "type": "string", "enum": ["knots", "ms"] },
        "unitBS": { "type": "string", "enum": ["knots", "ms"] }
      },
      "required": ["boatName", "unitTWS", "unitBS"]
    },
    "tws_grid": {
      "type": "array",
      "items": { "type": "number" },
      "minItems": 2
    },
    "twa_grid": {
      "type": "array",
      "items": { "type": "number" },
      "minItems": 2
    },
    "configurations": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "sail_id": { "type": "string" },
          "description": { "type": "string" },
          "matrix": {
            "type": "array",
            "description": "2D array of Boat Speeds indexed as matrix[twa_index][tws_index]",
            "items": {
              "type": "array",
              "items": { "type": "number" }
            }
          }
        },
        "required": ["sail_id", "matrix"]
      }
    }
  },
  "required": ["metadata", "tws_grid", "twa_grid", "configurations"]
}

4. Standardized CSV Format
For flat-file analytical pipelines, the CSV format must use a comma (,) separator with a standard header row:
```csv
TWA,TWS,BS,SailID
40,6,4.2,J1
40,8,5.3,J1
50,6,4.8,J1
50,8,5.9,J1
```

Rules: Every combination of TWA and TWS present in the polar grid must be flattened into rows, or the matrix CSV format (matching .pol rows/columns with comma separation) can be accepted if explicitly declared via configuration.

5. VMG Tables and Crossover Matrices
Optimal VMG Angles
Routing algorithms require optimal beating and running angles to calculate tacking/gybing points.
Definition: For each TWS in the grid, the optimal upwind VMG angle ($\text{TWA}_{\text{beat}}$) maximizes $\text{BS} \cdot \cos(\text{TWA})$, and optimal downwind VMG angle ($\text{TWA}_{\text{run}}$) maximizes $\text{BS} \cdot |\cos(\text{TWA})|$.
Storage Structure: Stored as supplementary arrays aligned with the TWS axis: $$\text{VMG\_Beat\_Table} = [\text{TWS}_i: (\text{TWA}_{\text{opt}}, \text{VMG}_{\text{max}})]$$
Sail Crossovers Matrix
Sail crossover boundaries define when to transition between headsails/spinnakers based on TWS and TWA.
Representation: A lookup table mapping each (TWS, TWA) coordinate tuple to the optimal sail_id string matching the ORC configuration list, preventing invalid sail usage (e.g., carrying a heavy spinnaker in 35 knots of TWS).

6. Dead Zone, Tapering, and Low-Wind Scaling Rules
Dead Zone (No-Sail Zone):
Defined between 0 and $\text{TWA}_{\text{dead}}$, where $\text{TWA}_{\text{dead}}$ is typically a function of TWS (ranging from 40 in light air to 52 in heavy air).
Within this zone, $\text{BS} = 0$, and routing engines force tacking maneuvers.
Low-Wind Quadratic Scaling:
For TWS values below the lowest defined grid column ($\text{TWS}_{\min}$), boat speed drops off quadratically to model aerodynamic stall and viscous drag: $$\text{BS}(\text{TWS}) = \text{BS}(\text{TWS}_{\min}) \cdot \left(\frac{\text{TWS}}{\text{TWS}_{\min}}\right)^2 \quad \text{for } \text{TWS} < \text{TWS}_{\min}$$
If $\text{TWS} = 0$, $\text{BS} = 0$.
