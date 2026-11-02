> const tx = await aeonianClient.openPosition({
>   market: SOL_USD_MARKET,
>   side: PositionSide.Long,
>   collateralAmount: new BN(10_000_000_000), // 10 SOL
>   leverage: 20,
>   slippageToleranceBps: 5,
>   oracleAccounts: [pythSolUsd, switchboardSolUsd],
> });
>