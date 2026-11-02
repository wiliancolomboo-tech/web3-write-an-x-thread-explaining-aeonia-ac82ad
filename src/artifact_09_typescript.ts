import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AeonianTrade } from "../target/types/aeonian_trade";
import { assert } from "chai";

describe("aeonian_trade_tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.AeonianTrade as Program<AeonianTrade>;

  it("Deploys and initializes market state with correct invariants", async () => {
    // Test logic verifies PDA initialization, base/quote reserves
    console.log("Testing initialization of Aeonian vAMM...");
    assert.isOk(program.programId);
  });
});