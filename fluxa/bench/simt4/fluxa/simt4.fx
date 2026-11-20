// A four-lane SIMT core.
//
// The front end executes one instruction at a time so that all four lanes
// remain in lockstep.  It owns the active mask and the stack of masks used
// by IF/ELSE/ENDIF.  Each lane owns its register file.  The memory task owns
// the shared sixteen-word memory and visits requesting lanes in lane order.

const LANE_BITS: [u8; 4] = [1, 2, 4, 8];

// Execute one instruction in a lane.  The command's final field says whether
// this lane is active for that instruction.
task Lane(
    command: in chan<(u8, u8, u8, u8, i32, u8)>,
    done: out chan<u8>,
    memory_request: out chan<(u8, i32, i32)>,
    memory_response: in chan<i32>,
    lane_id: u8
) {
    let mut regs: [i32; 8] = [0; 8];

    for instruction in command {
        let (op, rd, a, b, imm, active) = instruction;

        if op == 9 {
            // IF asks the front end for this lane's predicate.  The front end
            // combines the four predicates and maintains the mask stack.
            let condition: u8 = if regs[a] != 0 { 1 } else { 0 };
            done.send(condition);
        } else {
            if active != 0 {
                if op == 1 {
                    regs[rd] = imm;
                } else if op == 2 {
                    regs[rd] = lane_id as i32;
                } else if op == 3 {
                    regs[rd] = regs[a] + regs[b];
                } else if op == 4 {
                    regs[rd] = regs[a] - regs[b];
                } else if op == 5 {
                    regs[rd] = regs[a] * regs[b];
                } else if op == 12 {
                    regs[rd] = regs[a] & imm;
                } else if op == 8 {
                    regs[rd] = if regs[a] < regs[b] { 1 } else { 0 };
                } else if op == 6 {
                    let address = regs[a] + imm;
                    memory_request.send((op, address, 0));
                    regs[rd] = memory_response.recv();
                } else if op == 7 {
                    let address = regs[a] + imm;
                    let data = regs[b];
                    memory_request.send((op, address, data));
                    let acknowledgement = memory_response.recv();
                }
            }

            // Zero is an ordinary completion.  IF uses the same channel for
            // its predicate response.
            done.send(0);
        }
    }
}

// Own the shared memory.  For a load or store, active lanes are serviced in
// the fixed order 0, 1, 2, 3.  On HALT the memory image is sent to y.
task Memory(
    m: in chan<i32>,
    command: in chan<(u8, u8)>,
    request0: in chan<(u8, i32, i32)>,
    request1: in chan<(u8, i32, i32)>,
    request2: in chan<(u8, i32, i32)>,
    request3: in chan<(u8, i32, i32)>,
    response0: out chan<i32>,
    response1: out chan<i32>,
    response2: out chan<i32>,
    response3: out chan<i32>,
    y: out chan<i32>
) {
    let mut memory: [i32; 16] = [0; 16];

    for i in 0..16 {
        memory[i] = m.recv();
    }

    for (kind, active_mask) in command {
        if kind == 0 {
            for i in 0..16 {
                y.send(memory[i]);
            }
            y.close();
            return;
        }

        if active_mask & LANE_BITS[0] != 0 {
            let (request_kind, address, data) = request0.recv();
            assert request_kind == kind;
            assert address >= 0 && address < 16;
            if kind == 6 {
                response0.send(memory[address as u8]);
            } else {
                memory[address as u8] = data;
                response0.send(0);
            }
        }
        if active_mask & LANE_BITS[1] != 0 {
            let (request_kind, address, data) = request1.recv();
            assert request_kind == kind;
            assert address >= 0 && address < 16;
            if kind == 6 {
                response1.send(memory[address as u8]);
            } else {
                memory[address as u8] = data;
                response1.send(0);
            }
        }
        if active_mask & LANE_BITS[2] != 0 {
            let (request_kind, address, data) = request2.recv();
            assert request_kind == kind;
            assert address >= 0 && address < 16;
            if kind == 6 {
                response2.send(memory[address as u8]);
            } else {
                memory[address as u8] = data;
                response2.send(0);
            }
        }
        if active_mask & LANE_BITS[3] != 0 {
            let (request_kind, address, data) = request3.recv();
            assert request_kind == kind;
            assert address >= 0 && address < 16;
            if kind == 6 {
                response3.send(memory[address as u8]);
            } else {
                memory[address as u8] = data;
                response3.send(0);
            }
        }
    }
}

// Decode the instruction stream, broadcast each instruction, and maintain
// the four-bit active mask and its branch stack.
task FrontEnd(
    ins: in chan<(u8, u8, u8, u8, i32)>,
    command0: out chan<(u8, u8, u8, u8, i32, u8)>,
    command1: out chan<(u8, u8, u8, u8, i32, u8)>,
    command2: out chan<(u8, u8, u8, u8, i32, u8)>,
    command3: out chan<(u8, u8, u8, u8, i32, u8)>,
    done0: in chan<u8>,
    done1: in chan<u8>,
    done2: in chan<u8>,
    done3: in chan<u8>,
    memory_command: out chan<(u8, u8)>
) {
    let mut active_mask: u8 = 15;
    let mut mask_stack: [u8; 64] = [0; 64];
    let mut stack_depth: u8 = 0;

    for instruction in ins {
        let (op, rd, a, b, imm) = instruction;

        if op == 6 || op == 7 {
            // Tell memory which lanes will issue a request.  Memory then
            // visits those lanes in ascending lane order.
            memory_command.send((op, active_mask));
        }

        if op == 9 {
            // Broadcast IF before evaluating its predicates.
            command0.send((op, rd, a, b, imm, active_mask & LANE_BITS[0]));
            command1.send((op, rd, a, b, imm, active_mask & LANE_BITS[1]));
            command2.send((op, rd, a, b, imm, active_mask & LANE_BITS[2]));
            command3.send((op, rd, a, b, imm, active_mask & LANE_BITS[3]));

            let condition0 = done0.recv();
            let condition1 = done1.recv();
            let condition2 = done2.recv();
            let condition3 = done3.recv();
            let condition_mask = condition0 + 2 * condition1 + 4 * condition2 + 8 * condition3;

            assert stack_depth < 64;
            mask_stack[stack_depth] = active_mask;
            stack_depth = stack_depth + 1;
            active_mask = active_mask & condition_mask;
        } else {
            // Broadcast the instruction and wait until every lane has
            // completed it.  This barrier also orders memory requests.
            command0.send((op, rd, a, b, imm, active_mask & LANE_BITS[0]));
            command1.send((op, rd, a, b, imm, active_mask & LANE_BITS[1]));
            command2.send((op, rd, a, b, imm, active_mask & LANE_BITS[2]));
            command3.send((op, rd, a, b, imm, active_mask & LANE_BITS[3]));

            let completion0 = done0.recv();
            let completion1 = done1.recv();
            let completion2 = done2.recv();
            let completion3 = done3.recv();

            if op == 10 {
                // ELSE activates the lanes which took the other side of the
                // just-finished IF.
                assert stack_depth > 0;
                let pushed_mask = mask_stack[stack_depth - 1];
                active_mask = pushed_mask & (15 ^ active_mask);
            } else if op == 11 {
                // ENDIF restores the mask which was active before IF.
                assert stack_depth > 0;
                active_mask = mask_stack[stack_depth - 1];
                stack_depth = stack_depth - 1;
            }
        }

        if op == 0 {
            // All earlier instructions have completed.  Ask memory to emit
            // its final image and stop the front end.
            memory_command.send((0, active_mask));
            return;
        }
    }
}

// Top-level SIMT core.  The four lane tasks are independent register-file
// owners; the front end and memory task coordinate their execution.
task Simt4(
    m: in chan<i32>,
    ins: in chan<(u8, u8, u8, u8, i32)>,
    y: out chan<i32>
) {
    let (command0_tx, command0_rx) =
        chan::<(u8, u8, u8, u8, i32, u8)>(depth = 1);
    let (command1_tx, command1_rx) =
        chan::<(u8, u8, u8, u8, i32, u8)>(depth = 1);
    let (command2_tx, command2_rx) =
        chan::<(u8, u8, u8, u8, i32, u8)>(depth = 1);
    let (command3_tx, command3_rx) =
        chan::<(u8, u8, u8, u8, i32, u8)>(depth = 1);

    let (done0_tx, done0_rx) = chan::<u8>(depth = 1);
    let (done1_tx, done1_rx) = chan::<u8>(depth = 1);
    let (done2_tx, done2_rx) = chan::<u8>(depth = 1);
    let (done3_tx, done3_rx) = chan::<u8>(depth = 1);

    let (request0_tx, request0_rx) = chan::<(u8, i32, i32)>(depth = 1);
    let (request1_tx, request1_rx) = chan::<(u8, i32, i32)>(depth = 1);
    let (request2_tx, request2_rx) = chan::<(u8, i32, i32)>(depth = 1);
    let (request3_tx, request3_rx) = chan::<(u8, i32, i32)>(depth = 1);

    let (response0_tx, response0_rx) = chan::<i32>(depth = 1);
    let (response1_tx, response1_rx) = chan::<i32>(depth = 1);
    let (response2_tx, response2_rx) = chan::<i32>(depth = 1);
    let (response3_tx, response3_rx) = chan::<i32>(depth = 1);

    let (memory_command_tx, memory_command_rx) = chan::<(u8, u8)>(depth = 1);

    spawn FrontEnd(
        ins,
        command0_tx, command1_tx, command2_tx, command3_tx,
        done0_rx, done1_rx, done2_rx, done3_rx,
        memory_command_tx
    );

    spawn Lane(command0_rx, done0_tx, request0_tx, response0_rx, 0);
    spawn Lane(command1_rx, done1_tx, request1_tx, response1_rx, 1);
    spawn Lane(command2_rx, done2_tx, request2_tx, response2_rx, 2);
    spawn Lane(command3_rx, done3_tx, request3_tx, response3_rx, 3);

    spawn Memory(
        m,
        memory_command_rx,
        request0_rx, request1_rx, request2_rx, request3_rx,
        response0_tx, response1_tx, response2_tx, response3_tx,
        y
    );
}

test simt4_saxpy_and_divergent_branch {
    let (tm, rm) = chan::<i32>(depth = 32);
    let (ti, ri) = chan::<(u8, u8, u8, u8, i32)>(depth = 32);
    let (ty, ry) = chan::<i32>(depth = 32);

    spawn Simt4(rm, ri, ty);

    let initial_memory = [
        2, -5, 7, 100,
        1, 1, -1, -300,
        0, 0, 0, 0,
        0, 0, 0, 0
    ];
    for value in initial_memory {
        tm.send(value);
    }
    tm.close();

    let instructions = [
        (2, 1, 0, 0, 0),
        (1, 2, 0, 0, 3),
        (6, 3, 1, 0, 0),
        (6, 4, 1, 0, 4),
        (5, 5, 2, 3, 0),
        (3, 5, 5, 4, 0),
        (7, 0, 1, 5, 4),
        (12, 6, 1, 0, 1),
        (9, 0, 6, 0, 0),
        (1, 7, 0, 0, 10),
        (5, 7, 7, 1, 0),
        (7, 0, 1, 7, 8),
        (10, 0, 0, 0, 0),
        (1, 7, 0, 0, 0),
        (4, 7, 7, 1, 0),
        (7, 0, 1, 7, 8),
        (11, 0, 0, 0, 0),
        (0, 0, 0, 0, 0)
    ];
    for instruction in instructions {
        ti.send(instruction);
    }
    ti.close();

    let expected = [
        2, -5, 7, 100,
        7, -14, 20, 0,
        0, 10, -2, 30,
        0, 0, 0, 0
    ];
    for value in expected {
        assert ry.recv() == value;
    }
}
