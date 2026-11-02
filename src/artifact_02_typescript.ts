const tx = await client.openPosition({
  market: solUsdcMarketPubkey,
  side: Side.Long,
  collateralAmount: new BN(1_000_000_000), // 1 SOL
  leverage: 10,
  slippageToleranceBps: 50,
});
await sendAndConfirmTransaction(connection, tx, [payer]);