npx ts-node -e "
import * as anchor from '@coral-xyz/anchor';
import { AeonianTrade } from './target/types/aeonian_trade';

async function monitor() {
  const provider = anchor.AnchorProvider.env();
  const program = anchor.workspace.AeonianTrade as anchor.Program<AeonianTrade>;
  console.log('⚡ Aeonian Keeper Daemon Active. Monitoring open positions...');
  
  program.addEventListener('PositionOpenedEvent', (event, slot) => {
    console.log('[SLOT ' + slot + '] New Position: Trader ' + event.trader.toBase58() + ' Size: ' + event.size.toString());
  });

  program.addEventListener('LiquidationEvent', (event, slot) => {
    console.log('🚨 [LIQUIDATION DETECTED] Trader: ' + event.trader.toBase58() + ' Bounty: ' + event.keeperFee.toString());
  });
}
monitor();
"